//! The examples compiled by dotnet/Ara.slnx are embedded in each engine version.
use serde::{Deserialize, Serialize};

const SOURCES: &[(&str, &str)] = &[
    (
        "Ara.Examples/ComponentExamples.cs",
        include_str!("../../../../dotnet/examples/Ara.Examples/ComponentExamples.cs"),
    ),
    (
        "Ara.CommandExamples/CommandExamples.cs",
        include_str!("../../../../dotnet/examples/Ara.CommandExamples/CommandExamples.cs"),
    ),
    (
        "Ara.Examples/GameplayCommands.cs",
        include_str!("../../../../dotnet/examples/Ara.Examples/GameplayCommands.cs"),
    ),
];

/// A compiled region, addressed by the documentation page it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsharpExample {
    /// Page ID (`types/Light` or `commands/entity.create`).
    pub page: String,
    /// Source relative to dotnet/examples.
    pub source: String,
    /// Region name in that source.
    pub region: String,
    /// C# source inside the region, with common indentation removed.
    pub code: String,
}

/// Every example shipped in this engine build, in source/region order.
pub fn csharp_examples() -> &'static [CsharpExample] {
    static EXAMPLES: std::sync::OnceLock<Vec<CsharpExample>> = std::sync::OnceLock::new();
    EXAMPLES.get_or_init(parse)
}

fn parse() -> Vec<CsharpExample> {
    let mut examples = Vec::new();
    for &(source, text) in SOURCES {
        let mut active: Option<(&str, Vec<&str>)> = None;
        for line in text.lines() {
            if let Some(region) = line.trim().strip_prefix("#region ") {
                assert!(active.is_none(), "nested example region in {source}");
                active = Some((region, Vec::new()));
            } else if line.trim() == "#endregion" {
                let (region, lines) = active.take().expect("example region start");
                let indent = lines
                    .iter()
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| line.len() - line.trim_start().len())
                    .min()
                    .unwrap_or(0);
                examples.push(CsharpExample {
                    page: region.to_owned(),
                    source: source.to_owned(),
                    region: region.to_owned(),
                    code: lines
                        .iter()
                        .map(|line| line.get(indent..).unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join("\n"),
                });
            } else if let Some((_, lines)) = &mut active {
                lines.push(line);
            }
        }
        assert!(active.is_none(), "unclosed example region in {source}");
    }
    examples
}

pub(super) fn append(body: &mut String, page: &str) {
    for example in csharp_examples().iter().filter(|e| e.page == page) {
        body.push_str(&format!(
            "\n## C# example\n\n```csharp\n{}\n```\n",
            example.code
        ));
    }
}
