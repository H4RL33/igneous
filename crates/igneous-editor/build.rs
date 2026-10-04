fn main() {
    glib_build_tools::compile_resources(
        &["data"],
        "data/igneous-editor.gresource.xml",
        "igneous-editor.gresource",
    );
}
