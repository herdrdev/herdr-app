use std::sync::LazyLock;
use syntect::parsing::SyntaxSet;

pub(super) static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| {
    syntect::dumps::from_uncompressed_data(include_bytes!("gomod.packdump"))
        .expect("bundled Go module syntax")
});

#[cfg(test)]
mod tests {
    use super::*;
    use syntect::{
        parsing::{ParseState, ScopeStack},
        util::LinesWithEndings,
    };

    fn scopes(text: &str) -> Vec<String> {
        let syntax = SYNTAX_SET.find_syntax_by_token("go.mod").unwrap();
        let mut parser = ParseState::new(syntax);
        let mut stack = ScopeStack::new();
        let mut result = Vec::new();
        for line in LinesWithEndings::from(text) {
            let mut cursor = 0;
            for (index, operation) in parser.parse_line(line, &SYNTAX_SET).unwrap() {
                result.extend((cursor..index).map(|_| stack.scopes.last().unwrap().build_string()));
                stack.apply(&operation).unwrap();
                cursor = index;
            }
            result
                .extend((cursor..line.len()).map(|_| stack.scopes.last().unwrap().build_string()));
        }
        result
    }

    fn assert_scope(text: &str, token: &str, expected: &str) {
        let offset = text.find(token).unwrap();
        let scopes = scopes(text);
        assert!(
            scopes[offset..offset + token.len()]
                .iter()
                .all(|scope| scope.starts_with(expected)),
            "{token:?}: {:?}, wanted {expected}",
            &scopes[offset..offset + token.len()]
        );
    }

    #[test]
    fn module_paths_versions_and_comments_have_distinct_scopes() {
        let text = "module example.com/app\n\ngo 1.27.4\nrequire (\n example.com/lib/v2 v2.1.0-beta.1+incompatible // indirect\n)\n";
        assert_scope(text, "module", "keyword");
        assert_scope(text, "example.com/app", "string");
        assert_scope(text, "1.27.4", "constant");
        assert_scope(text, "example.com/lib/v2", "string");
        assert_scope(text, "v2.1.0-beta.1+incompatible", "constant");
        assert_scope(text, "indirect", "comment");
        assert_scope(text, ")", "punctuation");
    }

    #[test]
    fn replacements_and_quoted_paths_do_not_leak_into_the_next_line() {
        let text = "replace (\n example.com/lib => \"../local folder/lib\"\n example.com/other v1.0.0 => example.com/fork v0.0.0-20260101120000-abcdef123456\n)\nrequire example.com/next v1.2.3";
        assert_scope(text, "=>", "keyword.operator");
        assert_scope(text, "\"../local folder/lib\"", "string");
        assert_scope(text, "v0.0.0-20260101120000-abcdef123456", "constant");
        assert_scope(text, "require", "keyword");
        assert_scope(text, "v1.2.3", "constant");
    }

    #[test]
    fn modern_directives_and_retraction_ranges() {
        let text = "toolchain go1.27.4\ngodebug (\n panicnil=1\n)\ntool example.com/cmd/tool\nretract (\n [v1.0.0, v1.1.0] // broken releases\n v1.2.0\n)\nexclude example.com/lib v1.3.0\n";
        for token in ["toolchain", "godebug", "retract", "exclude"] {
            assert_scope(text, token, "keyword");
        }
        assert_scope("tool example.com/cmd/tool", "tool", "keyword");
        for token in ["go1.27.4", "v1.0.0", "v1.1.0", "v1.2.0", "v1.3.0"] {
            assert_scope(text, token, "constant");
        }
        assert_scope(text, "example.com/cmd/tool", "string");
        assert_scope(text, "broken releases", "comment");
        assert_scope(text, "panicnil=1", "variable");
    }

    #[test]
    fn comments_and_paths_are_not_keywords() {
        let text = "// require example.com/fake v1.0.0\r\nmodule example.com/require\r\nreplace example.com/lib => \"../a//b\\\"c\"\r\ngo 1.27.4\r\n";
        assert_scope(text, "require example.com/fake", "comment");
        assert_scope(text, "example.com/require", "string");
        assert_scope(text, "a//b", "string");
        assert_scope(text, "1.27.4", "constant");
    }

    #[test]
    fn incomplete_input_does_not_panic_or_hide_later_directives() {
        for text in [
            "require (\n",
            "replace example.com/lib =>",
            "replace example.com/lib => \"unfinished\ngo 1.27.4\n",
        ] {
            assert_eq!(scopes(text).len(), text.len());
        }
        assert_scope(
            "replace example.com/lib => \"unfinished\ngo 1.27.4\n",
            "go",
            "keyword",
        );
    }
}
