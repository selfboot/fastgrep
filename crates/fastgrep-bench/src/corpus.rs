/// Test corpus generation.

use std::fs;
use std::path::Path;

use anyhow::Result;

pub fn prepare(corpus_name: &str, output_dir: &str) -> Result<()> {
    let output = Path::new(output_dir);
    fs::create_dir_all(output)?;

    match corpus_name {
        "small" => generate_small(output),
        "medium" => generate_medium(output),
        "linux-kernel" => {
            eprintln!("To prepare the linux-kernel corpus, clone it manually:");
            eprintln!("  git clone --depth 1 https://github.com/torvalds/linux.git {}/linux-kernel", output_dir);
            Ok(())
        }
        _ => anyhow::bail!("Unknown corpus: {}. Available: small, medium, linux-kernel", corpus_name),
    }
}

fn generate_small(output: &Path) -> Result<()> {
    let dir = output.join("small");
    fs::create_dir_all(&dir)?;

    for i in 0..100 {
        let content = generate_rust_file(i);
        fs::write(dir.join(format!("file_{:04}.rs", i)), content)?;
    }

    eprintln!("Generated small corpus: 100 files at {}", dir.display());
    Ok(())
}

fn generate_medium(output: &Path) -> Result<()> {
    let dir = output.join("medium");

    // Create subdirectories
    for subdir in &["src", "src/models", "src/handlers", "src/utils", "tests"] {
        fs::create_dir_all(dir.join(subdir))?;
    }

    let mut count = 0;
    for subdir in &["src", "src/models", "src/handlers", "src/utils", "tests"] {
        for i in 0..2000 {
            let content = generate_rust_file(count);
            fs::write(
                dir.join(subdir).join(format!("file_{:06}.rs", i)),
                content,
            )?;
            count += 1;
        }
    }

    eprintln!("Generated medium corpus: {} files at {}", count, dir.display());
    Ok(())
}

fn generate_rust_file(seed: usize) -> String {
    let type_names = ["HashMap", "Vec", "String", "Option", "Result", "BTreeMap"];
    let trait_names = ["Display", "Debug", "Clone", "Default", "Iterator"];

    let type_name = type_names[seed % type_names.len()];
    let trait_name = trait_names[seed % trait_names.len()];

    format!(
        r#"//! Auto-generated test file {seed}
// SPDX-License-Identifier: MIT

use std::collections::{type_name};

/// A struct for testing.
pub struct TestStruct{seed} {{
    pub data: {type_name}<String, Vec<u8>>,
    pub name: String,
}}

impl TestStruct{seed} {{
    pub fn new(name: &str) -> Self {{
        // TODO: add validation
        Self {{
            data: {type_name}::new(),
            name: name.to_string(),
        }}
    }}

    pub fn process(&self) -> Result<(), Box<dyn std::error::Error>> {{
        // FIXME: handle edge cases
        for (key, value) in &self.data {{
            println!("{{key}}: {{value:?}}");
        }}
        Ok(())
    }}
}}

impl {trait_name} for TestStruct{seed} {{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{
        write!(f, "TestStruct{seed}({{}})", self.name)
    }}
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn test_new() {{
        let s = TestStruct{seed}::new("test");
        assert_eq!(s.name, "test");
    }}
}}
"#,
    )
}
