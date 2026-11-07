fn main() {
    // fluent: only for std-widgets' `Palette` (OS dark / light detection); every widget is our own
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("slint build failed");
    println!("cargo:rerun-if-changed=i18n");
}
