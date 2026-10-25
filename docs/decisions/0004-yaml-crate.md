# 0004 YAML parser for USTX: serde-saphyr

USTX files are YAML. The locked stack lists no YAML crate. serde_yaml is
deprecated and unmaintained, and serde_yml had soundness problems.
serde-saphyr (2.0.0, released October 2026) is a maintained serde front end
over the pure-Rust saphyr parser, with no unsafe C dependency. We only need
deserialization, which it covers.
