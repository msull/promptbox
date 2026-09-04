//! Projects: vocabulary and, later, correction rules. Currently a built-in
//! placeholder list that is not persisted.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub vocabulary: Vec<String>,
}

impl Project {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            vocabulary: Vec::new(),
        }
    }
}

/// Built-in placeholders until projects are persisted.
#[must_use]
pub fn placeholder_projects() -> Vec<Project> {
    vec![
        Project::new("Default"),
        Project {
            name: "Acme".to_owned(),
            vocabulary: ["Acme", "Univer Sheets", "FastHTML", "Pydantic", "DynamoDB"]
                .map(str::to_owned)
                .to_vec(),
        },
    ]
}
