//! `peekaround` — the PeekAround media gallery server. Flags, environment variables and help
//! text mirror `packages/server/src/cli.ts`.

use pa_server::{RootSpec, Server, ServerOptions};
use std::path::{Path, PathBuf};

const HELP: &str = "PeekAround — media gallery server

Usage: peekaround [--root <dir>]... [options]

  -r, --root <dir>        library folder (repeatable, \"Name=dir\" allowed)   env PEEK_ROOTS
  -d, --data <dir>        data/cache directory (default ~/.peekaround)     env PEEK_DATA_DIR
  -p, --port <port>       port (default 8080)                             env PEEK_PORT
      --host <host>       bind address (default 0.0.0.0)                  env PEEK_HOST
      --readonly          disable uploads/edits/deletes                   env PEEK_READONLY
      --poll              poll for file changes (network shares)           env PEEK_WATCH_POLL
      --split-roots       one library per sub folder of each root          env PEEK_SPLIT_ROOTS
      --browse-root <dir> limit the \"add folder\" browser to these dirs      env PEEK_BROWSE_ROOTS
      --no-mdns           don't announce on the local network              env PEEK_MDNS=false
      --web <dir>         folder of the browser UI (index.html)            env PEEK_WEB_DIR
      --healthcheck       exit 0 when the server on this machine answers (Docker HEALTHCHECK)
";

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn boolean(v: Option<String>, def: bool) -> bool {
    match v {
        None => def,
        Some(v) => ["1", "true", "yes", "on"].contains(&v.to_lowercase().as_str()),
    }
}

/// "Photos=/media/photos,/media/videos" → [{name: Photos, path: /media/photos}, {path: /media/videos}]
fn parse_roots(list: &[String], split: bool) -> Vec<RootSpec> {
    list.iter()
        .flat_map(|s| {
            s.split([',', ';', '\n'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .map(|s| match s.find('=') {
            Some(i) if i > 0 && !(i == 1 && s.as_bytes()[0].is_ascii_alphabetic()) => RootSpec {
                name: Some(s[..i].to_string()),
                path: s[i + 1..].to_string(),
                split,
            },
            _ => RootSpec {
                name: None,
                path: s,
                split,
            },
        })
        .collect()
}

#[derive(Default)]
struct Args {
    root: Vec<String>,
    data: Option<String>,
    port: Option<String>,
    host: Option<String>,
    web: Option<String>,
    readonly: Option<bool>,
    poll: Option<bool>,
    no_mdns: bool,
    split_roots: Option<bool>,
    browse_root: Vec<String>,
    verbose: Option<bool>,
    healthcheck: bool,
    help: bool,
}

/// node:util parseArgs' message for an unknown option
fn unknown(opt: &str) -> String {
    format!("TypeError [ERR_PARSE_ARGS_UNKNOWN_OPTION]: Unknown option '{opt}'. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- \"{opt}\"")
}

/// "-p, " for options with a short alias
fn short_of(long: &str) -> &'static str {
    match long {
        "root" => "-r, ",
        "data" => "-d, ",
        "port" => "-p, ",
        _ => "",
    }
}

fn parse_args(argv: Vec<String>) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = argv.into_iter().peekable();
    while let Some(arg) = it.next() {
        if arg == "--" {
            break;
        }
        let (name, inline): (String, Option<String>) = if let Some(rest) = arg.strip_prefix("--") {
            match rest.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (rest.to_string(), None),
            }
        } else if let Some(rest) = arg.strip_prefix('-').filter(|r| !r.is_empty()) {
            let mut cs = rest.chars();
            let c = cs.next().unwrap_or(' ');
            let long = match c {
                'r' => "root",
                'd' => "data",
                'p' => "port",
                'v' => "verbose",
                'h' => "help",
                _ => return Err(unknown(&format!("-{c}"))),
            };
            let tail: String = cs.collect();
            (
                long.to_string(),
                if tail.is_empty() { None } else { Some(tail) },
            )
        } else {
            continue; // positionals are allowed (and ignored)
        };
        let mut value = |n: &str| -> Result<String, String> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => it
                    .next()
                    .ok_or_else(|| format!("TypeError [ERR_PARSE_ARGS_INVALID_OPTION_VALUE]: Option '{}--{n} <value>' argument missing", short_of(n))),
            }
        };
        match name.as_str() {
            "root" => a.root.push(value("root")?),
            "data" => a.data = Some(value("data")?),
            "port" => a.port = Some(value("port")?),
            "host" => a.host = Some(value("host")?),
            "web" => a.web = Some(value("web")?),
            "browse-root" => a.browse_root.push(value("browse-root")?),
            "readonly" => a.readonly = Some(true),
            "poll" => a.poll = Some(true),
            "no-mdns" => a.no_mdns = true,
            "split-roots" => a.split_roots = Some(true),
            "verbose" => a.verbose = Some(true),
            "healthcheck" => a.healthcheck = true,
            "help" => a.help = true,
            other => return Err(unknown(&format!("--{other}"))),
        }
    }
    Ok(a)
}

fn web_dir(a: &Args) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(w) = &a.web {
        c.push(w.into());
    }
    if let Some(w) = env("PEEK_WEB_DIR") {
        c.push(w.into());
    }
    if let Some(d) = &exe_dir {
        c.push(d.join("web"));
        c.push(d.join("../web"));
        c.push(d.join("../share/peekaround/web"));
    }
    c.into_iter().find(|d| d.join("index.html").is_file())
}

/// `GET /api/info` on this machine's server: true when it answers 200.
fn healthy(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::time::Duration;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    let req = format!("GET /api/info HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut head = [0u8; 12];
    s.read_exact(&mut head).is_ok() && head.ends_with(b" 200")
}

#[tokio::main]
async fn main() {
    let args = match parse_args(std::env::args().skip(1).collect()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if args.help {
        println!("{HELP}");
        return;
    }
    let verbose = args
        .verbose
        .unwrap_or_else(|| boolean(env("PEEK_VERBOSE"), false));
    if verbose {
        tracing_subscriber::fmt().with_target(false).init();
    }
    let web = web_dir(&args);
    let split = args
        .split_roots
        .unwrap_or_else(|| boolean(env("PEEK_SPLIT_ROOTS"), false));
    let mut list = args.root.clone();
    if let Some(r) = env("PEEK_ROOTS") {
        list.push(r);
    }
    let roots = parse_roots(&list, split);
    let home = env("HOME")
        .or_else(|| env("USERPROFILE"))
        .unwrap_or_else(|| "/".into());
    let data = args
        .data
        .clone()
        .or_else(|| env("PEEK_DATA_DIR"))
        .unwrap_or_else(|| {
            Path::new(&home)
                .join(".peekaround")
                .to_string_lossy()
                .into_owned()
        });
    let port_s = args
        .port
        .clone()
        .or_else(|| env("PEEK_PORT"))
        .unwrap_or_else(|| "8080".into());
    let Ok(port) = port_s.trim().parse::<u16>() else {
        eprintln!("Invalid port: {port_s}");
        std::process::exit(1);
    };
    if args.healthcheck {
        std::process::exit(if healthy(port) { 0 } else { 1 });
    }
    let mut browse: Vec<String> = args.browse_root.clone();
    if let Some(b) = env("PEEK_BROWSE_ROOTS") {
        browse.extend(
            b.split([',', ';'])
                .filter(|s| !s.is_empty())
                .map(String::from),
        );
    }
    if browse.is_empty() {
        browse = roots.iter().map(|r| r.path.clone()).collect();
        browse.push(home.clone());
    }
    let mut o = ServerOptions::new(data);
    o.port = port;
    o.host = args
        .host
        .clone()
        .or_else(|| env("PEEK_HOST"))
        .unwrap_or_else(|| "0.0.0.0".into());
    o.roots = roots;
    o.web_dir = web.clone();
    o.readonly = args
        .readonly
        .unwrap_or_else(|| boolean(env("PEEK_READONLY"), false));
    o.watch_poll = args
        .poll
        .unwrap_or_else(|| boolean(env("PEEK_WATCH_POLL"), false));
    o.mdns = if args.no_mdns {
        false
    } else {
        boolean(env("PEEK_MDNS"), true)
    };
    o.host_browse_roots = browse;
    o.logger = verbose;
    o.version = env("npm_package_version").unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let server = match Server::start(o).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let lan = pa_server::config::lan_urls(server.port);
    let net: Vec<String> = lan.iter().map(|u| format!("    Network: {u}")).collect();
    println!(
        "\n  ✦ PeekAround is running\n\n    Local:   {}\n{}\n{}",
        server.url,
        net.join("\n"),
        if web.is_some() {
            ""
        } else {
            "\n    (no browser UI: build it with crates/pa-web/build.sh and pass --web <dir>)\n"
        }
    );
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
    server.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots() {
        let r = parse_roots(
            &["Photos=/media/photos,/media/videos".into(), "C:\\x".into()],
            false,
        );
        assert_eq!(r[0].name.as_deref(), Some("Photos"));
        assert_eq!(r[0].path, "/media/photos");
        assert_eq!(r[1].name, None);
        assert_eq!(r[2].path, "C:\\x");
        let a = parse_args(vec![
            "-r".into(),
            "A=/a".into(),
            "--port=9".into(),
            "--no-mdns".into(),
        ])
        .unwrap();
        assert_eq!(a.root, vec!["A=/a"]);
        assert_eq!(a.port.as_deref(), Some("9"));
        assert!(a.no_mdns);
        assert!(parse_args(vec!["--bogus".into()]).is_err());
    }
}
