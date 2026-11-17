//! Rules on the .slint sources that the Slint compiler does not check.

/// Every `PopupWindow` sets its height before its first child. Without one, Slint sizes the popup
/// from its children's layout info, which is 0 for a `PopoverPanel` (its height is its own
/// binding): the popup draws, but every click on it counts as outside and only closes it.
#[test]
fn popups_have_a_height() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut bad = vec![];
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().is_none_or(|x| x != "slint") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            let lines: Vec<&str> = src.lines().collect();
            for (i, l) in lines.iter().enumerate() {
                if !l.trim_end().ends_with("PopupWindow {") {
                    continue;
                }
                // the popup's own properties: the lines up to its first child element
                let own = lines[i + 1..]
                    .iter()
                    .take_while(|l| !l.trim_end().ends_with('{'));
                if !own.into_iter().any(|l| l.trim_start().starts_with("height:")) {
                    bad.push(format!("{}:{}", p.display(), i + 1));
                }
            }
        }
    }
    assert!(bad.is_empty(), "PopupWindow without a height: {bad:#?}");
}
