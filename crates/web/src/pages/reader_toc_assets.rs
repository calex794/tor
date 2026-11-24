//! Compose TOC presentation code into the existing self-hosted bundles.
//! No additional route, request, script global or access-policy change.

pub(crate) fn append(js: &mut String, toc: &str) {
    js.push_str("\n(() => {\n\"use strict\";\n");
    js.push_str(include_str!("../../assets/newboo-toc-dialog.js"));
    js.push('\n');
    js.push_str(toc);
    js.push_str("\n})();\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_dialog_is_local_and_precedes_variant() {
        for toc in [
            include_str!("../../assets/newboo-free-toc.js"),
            include_str!("../../assets/newboo-paid-toc.js"),
            include_str!("../../assets/newboo-archive-toc.js"),
        ] {
            let mut js = String::from("/* previous script */");
            append(&mut js, toc);
            assert_eq!(
                js,
                format!(
                    "/* previous script */\n(() => {{\n\"use strict\";\n{}\n{}\n}})();\n",
                    include_str!("../../assets/newboo-toc-dialog.js"),
                    toc
                )
            );
            assert_eq!(js.matches("function bindTocDialog(").count(), 1);
        }
    }
}
