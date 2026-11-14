use std::{env, fs, path::Path};
use syntect::parsing::{SyntaxDefinition, SyntaxSetBuilder};

#[cfg(test)]
#[path = "../../../vendor/gpui-kit/crates/shell/src/syntax/gomod.rs"]
mod gomod;

fn main() {
    let directory =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/gpui-kit/crates/shell/src/syntax");
    let source = fs::read_to_string(directory.join("Gomod.sublime-syntax")).unwrap();
    let definition = SyntaxDefinition::load_from_str(&source, true, Some("Gomod")).unwrap();
    let mut builder = SyntaxSetBuilder::new();
    builder.add(definition);
    let set = builder.build();
    let output = directory.join("gomod.packdump");
    if env::args().any(|arg| arg == "--check") {
        let temporary =
            env::temp_dir().join(format!("herdr-gomod-{}.packdump", std::process::id()));
        syntect::dumps::dump_to_uncompressed_file(&set, &temporary).unwrap();
        let bytes = fs::read(&temporary).unwrap();
        fs::remove_file(temporary).unwrap();
        assert_eq!(
            fs::read(output).unwrap(),
            bytes,
            "regenerate gomod.packdump"
        );
    } else {
        syntect::dumps::dump_to_uncompressed_file(&set, output).unwrap();
    }
}
