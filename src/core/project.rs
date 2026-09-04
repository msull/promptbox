//! Projects: vocabulary and correction rules. Milestone 1 only needs a
//! selectable placeholder list; persistence and rules arrive in Milestone 5.

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
