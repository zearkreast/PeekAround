//! XMP packet parser with the semantics of exifr 7 (`segment-parsers/xmp.mjs`).
//!
//! exifr does not use an XML parser: it finds `rdf:Description` tags and their attributes / child
//! elements with regular expressions, numbers nested `rdf:li|Seq|Bag|Alt|Description` tags, converts
//! values (`"8000"` → 8000, `"True"` → true), turns `rdf:Seq/Bag/Alt` into arrays (one item → the item)
//! and flattens every namespace into the output by local name. Reproducing that (instead of a strict XML
//! parser such as quick-xml) keeps the extracted values identical, including on malformed packets.

use indexmap::IndexMap;
use pa_core::js;

use crate::value::{Tags, Val};

const NESTED: [&str; 5] = ["rdf:li", "rdf:Seq", "rdf:Bag", "rdf:Alt", "rdf:Description"];

/// `idNestedTags`: `<rdf:li` → `<rdf:li#1`, `</rdf:li` → `</rdf:li#1` (stack per tag name)
fn id_nested_tags(s: &str) -> String {
    let mut counts = [0usize; 5];
    let mut stacks: [Vec<usize>; 5] = Default::default();
    let mut out = String::with_capacity(s.len() + 64);
    let b = s.as_bytes();
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        if b[i] == b'<' || b[i] == b'/' {
            if let Some(k) = NESTED.iter().position(|t| s[i + 1..].starts_with(t)) {
                let end = i + 1 + NESTED[k].len();
                out.push_str(&s[last..end]);
                if b[i] == b'<' {
                    counts[k] += 1;
                    stacks[k].push(counts[k]);
                    out.push_str(&format!("#{}", counts[k]));
                } else {
                    match stacks[k].pop() {
                        Some(id) => out.push_str(&format!("#{id}")),
                        None => out.push_str("#undefined"),
                    }
                }
                last = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    out
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// one JS-whitespace char at byte `i` → its length
fn space_at(s: &str, i: usize) -> Option<usize> {
    s[i..]
        .chars()
        .next()
        .filter(|c| js::is_space(*c))
        .map(char::len_utf8)
}

fn skip_spaces(s: &str, mut i: usize) -> usize {
    while i < s.len() {
        match space_at(s, i) {
            Some(l) => i += l,
            None => break,
        }
    }
    i
}

/// `("[^"]*"|'[^']*')` at `i` → end
fn quoted(s: &str, i: usize) -> Option<usize> {
    let q = *s.as_bytes().get(i)?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    s[i + 1..].find(q as char).map(|p| i + 1 + p + 1)
}

fn run(s: &str, i: usize, ok: impl Fn(u8) -> bool) -> usize {
    let b = s.as_bytes();
    let mut j = i;
    while j < b.len() && ok(b[j]) {
        j += 1;
    }
    j
}

#[derive(Debug, Clone)]
struct Attr {
    ns: String,
    name: String,
    value: Val,
}

#[derive(Debug, Clone)]
struct Tag {
    ns: String,
    name: String,
    attrs: Vec<Attr>,
    children: Vec<Tag>,
    value: Val,
}

/// `XmlAttr.findAll`: `([a-zA-Z0-9-]+):([a-zA-Z0-9-]+)=("[^"]*"|'[^']*')` anywhere in the string
fn find_attrs(s: &str) -> Vec<Attr> {
    let cls = |c: u8| c.is_ascii_alphanumeric() || c == b'-';
    let b = s.as_bytes();
    let mut out = vec![];
    let mut p = 0;
    while p < b.len() {
        let try_at = || -> Option<(Attr, usize)> {
            let ns_end = run(s, p, cls);
            if ns_end == p || b.get(ns_end) != Some(&b':') {
                return None;
            }
            let nm_end = run(s, ns_end + 1, cls);
            if nm_end == ns_end + 1 || b.get(nm_end) != Some(&b'=') {
                return None;
            }
            let end = quoted(s, nm_end + 1)?;
            let raw = &s[nm_end + 2..end - 1];
            Some((
                Attr {
                    ns: s[p..ns_end].into(),
                    name: s[ns_end + 1..nm_end].into(),
                    value: normalize_value(Some(raw)),
                },
                end,
            ))
        };
        match try_at() {
            Some((a, end)) => {
                out.push(a);
                p = end;
            }
            None => {
                p += 1;
                while p < b.len() && !s.is_char_boundary(p) {
                    p += 1;
                }
            }
        }
    }
    out
}

/// `XmlTag.findAll(xml, ns, name)`: `<(ns):(name)(#\d+)?((\s+?[\w\d-:]+=("[^"]*"|'[^']*'))*\s*)(\/>|>([\s\S]*?)<\/\1:\2\3>)`
fn find_tags(s: &str, ns: Option<&str>, name: Option<&str>) -> Vec<Tag> {
    let mut out = vec![];
    let mut pos = 0;
    while let Some(off) = s[pos..].find('<') {
        let i = pos + off;
        match match_tag(s, i, ns, name) {
            Some((t, end)) => {
                out.push(t);
                pos = end;
            }
            None => pos = i + 1,
        }
    }
    out
}

fn match_tag(
    s: &str,
    i: usize,
    want_ns: Option<&str>,
    want_name: Option<&str>,
) -> Option<(Tag, usize)> {
    let b = s.as_bytes();
    let part = |j: usize, want: Option<&str>| -> Option<usize> {
        match want {
            Some(w) => s[j..].starts_with(w).then_some(j + w.len()),
            None => {
                let e = run(s, j, |c| is_word(c) || c == b'-');
                (e > j).then_some(e)
            }
        }
    };
    let ns_start = i + 1;
    let ns_end = part(ns_start, want_ns)?;
    if b.get(ns_end) != Some(&b':') {
        return None;
    }
    let name_end = part(ns_end + 1, want_name)?;
    let mut j = name_end;
    let id = if b.get(j) == Some(&b'#') {
        let e = run(s, j + 1, |c| c.is_ascii_digit());
        if e > j + 1 {
            let id = &s[j..e];
            j = e;
            id
        } else {
            ""
        }
    } else {
        ""
    };
    let attr_start = j;
    // (\s+?[\w\d-:]+=("[^"]*"|'[^']*'))*
    loop {
        let k = skip_spaces(s, j);
        if k == j {
            break;
        }
        let ne = run(s, k, |c| is_word(c) || c == b'-' || c == b':');
        if ne == k || b.get(ne) != Some(&b'=') {
            break;
        }
        match quoted(s, ne + 1) {
            Some(e) => j = e,
            None => break,
        }
    }
    j = skip_spaces(s, j);
    let attr_string = &s[attr_start..j];
    let (ns, name) = (&s[ns_start..ns_end], &s[ns_end + 1..name_end]);
    if s[j..].starts_with("/>") {
        return Some((Tag::new(ns, name, Some(attr_string), None), j + 2));
    }
    if b.get(j) != Some(&b'>') {
        return None;
    }
    let close = format!("</{ns}:{name}{id}>");
    let inner_start = j + 1;
    let p = s[inner_start..].find(&close)?;
    let inner = &s[inner_start..inner_start + p];
    Some((
        Tag::new(ns, name, Some(attr_string), Some(inner)),
        inner_start + p + close.len(),
    ))
}

impl Tag {
    fn new(ns: &str, name: &str, attr_string: Option<&str>, inner: Option<&str>) -> Tag {
        let attrs = attr_string
            .filter(|a| !a.is_empty())
            .map(find_attrs)
            .unwrap_or_default();
        let children = inner
            .filter(|x| !x.is_empty())
            .map(|x| find_tags(x, None, None))
            .unwrap_or_default();
        let value = if children.is_empty() {
            normalize_value(inner)
        } else {
            Val::Undef
        };
        Tag {
            ns: ns.into(),
            name: name.into(),
            attrs,
            children,
            value,
        }
    }

    fn props(&self) -> impl Iterator<Item = (&str, &str, Val)> {
        self.attrs
            .iter()
            .map(|a| (a.ns.as_str(), a.name.as_str(), a.value.clone()))
            .chain(
                self.children
                    .iter()
                    .map(|c| (c.ns.as_str(), c.name.as_str(), c.serialize())),
            )
    }

    fn is_list(&self) -> bool {
        self.ns == "rdf" && matches!(self.name.as_str(), "Seq" | "Bag" | "Alt")
    }

    fn serialize(&self) -> Val {
        if self.attrs.is_empty() && self.children.is_empty() {
            return self.value.clone(); // primitive (or undefined)
        }
        if self.children.len() == 1 && self.children[0].is_list() {
            return self.children[0].serialize();
        }
        if self.is_list() {
            let mut items: Vec<Val> = self.children.iter().map(Tag::serialize).collect();
            return if items.len() == 1 {
                items.remove(0)
            } else {
                Val::Arr(items)
            };
        }
        if self.ns == "rdf"
            && self.name == "li"
            && self.children.len() == 1
            && self.attrs.is_empty()
        {
            return self.children[0].serialize();
        }
        let mut out = IndexMap::new();
        for (_, name, v) in self.props() {
            if !v.is_undef() {
                out.insert(name.to_string(), v);
            }
        }
        if !self.value.is_undef() {
            out.insert("value".into(), self.value.clone());
        }
        if out.is_empty() {
            Val::Undef
        } else {
            Val::Obj(out)
        }
    }
}

/// exifr `normalizeValue`
fn normalize_value(v: Option<&str>) -> Val {
    let Some(s) = v else { return Val::Undef };
    if s == "null" || s == "undefined" || js::trim(s).is_empty() {
        return Val::Undef;
    }
    let n = js::number(s);
    if !n.is_nan() {
        return Val::Num(n);
    }
    match s.to_lowercase().as_str() {
        "true" => Val::Bool(true),
        "false" => Val::Bool(false),
        _ => Val::Str(js::trim(s).to_string()),
    }
}

/// Parse an XMP packet into namespace → (local name → value), in exifr's order.
pub(crate) fn parse(xmp: &str) -> IndexMap<String, IndexMap<String, Val>> {
    let s = id_nested_tags(xmp);
    let mut tags = find_tags(&s, Some("rdf"), Some("Description"));
    if tags.is_empty() {
        tags.push(Tag::new("rdf", "Description", None, Some(&s)));
    }
    let mut out: IndexMap<String, IndexMap<String, Val>> = IndexMap::new();
    for t in &tags {
        for (ns, name, v) in t.props() {
            let nso = out.entry(ns.to_string()).or_default();
            if !v.is_undef() {
                nso.insert(name.to_string(), v);
            }
        }
    }
    out.retain(|_, m| !m.is_empty());
    out
}

/// `assignToOutput` with `mergeOutput: true`: every namespace (but xmlns) is merged by local name.
pub(crate) fn merge_into(root: &mut Tags, xmp: &str) {
    for (ns, obj) in parse(xmp) {
        if ns == "xmlns" {
            continue;
        }
        for (k, v) in obj {
            root.insert(k, v);
        }
    }
}
