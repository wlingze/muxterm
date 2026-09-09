//! Workspace templates: create-time tab/pane layout and process defaults.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::protocol::layout::SplitDir;

/// Stable name used by Project records and Workspace open specs.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct TemplateName(String);

impl TemplateName {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(anyhow!("template name 不能为空"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TemplateName {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for TemplateName {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for TemplateName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A create-time workspace layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceTemplate {
    pub name: TemplateName,
    #[serde(default)]
    pub tabs: Vec<TabTemplate>,
}

impl WorkspaceTemplate {
    pub fn validate(&self) -> Result<()> {
        if self.tabs.is_empty() {
            return Err(anyhow!("template {} 至少需要一个 tab", self.name));
        }
        let mut active_tabs = 0;
        for tab in &self.tabs {
            if tab.active {
                active_tabs += 1;
            }
            tab.layout.validate()?;
        }
        if active_tabs > 1 {
            return Err(anyhow!("template {} 最多只能有一个 active tab", self.name));
        }
        Ok(())
    }

    pub fn pane_count(&self) -> usize {
        self.tabs.iter().map(|tab| tab.layout.pane_count()).sum()
    }
}

/// One template tab and its nested pane layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TabTemplate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub layout: TemplateLayout,
    #[serde(default)]
    pub active: bool,
}

/// Recursive layout tree; a leaf is one pane to create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind")]
pub enum TemplateLayout {
    Pane(PaneTemplate),
    Split {
        dir: SplitDir,
        first: Box<TemplateLayout>,
        second: Box<TemplateLayout>,
    },
}

impl TemplateLayout {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Pane(pane) => pane.validate(),
            Self::Split { first, second, .. } => {
                first.validate()?;
                second.validate()
            }
        }
    }

    pub fn pane_count(&self) -> usize {
        match self {
            Self::Pane(_) => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }
}

/// Process and focus defaults for one template pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneTemplate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub focus: bool,
}

impl PaneTemplate {
    fn validate(&self) -> Result<()> {
        if self
            .command
            .as_deref()
            .is_some_and(|command| command.trim().is_empty())
        {
            return Err(anyhow!("template pane command 不能为空"));
        }
        if self.env.keys().any(|key| key.trim().is_empty()) {
            return Err(anyhow!("template pane env key 不能为空"));
        }
        Ok(())
    }
}

/// Named template collection used by Config and Projects.
#[derive(Debug, Clone, Default)]
pub struct TemplateRegistry {
    templates: Vec<WorkspaceTemplate>,
}

impl TemplateRegistry {
    pub fn new(templates: Vec<WorkspaceTemplate>) -> Result<Self> {
        let mut registry = Self::default();
        for template in templates {
            registry.insert(template)?;
        }
        Ok(registry)
    }

    pub fn templates(&self) -> &[WorkspaceTemplate] {
        &self.templates
    }

    pub fn get(&self, name: &TemplateName) -> Option<&WorkspaceTemplate> {
        self.templates
            .iter()
            .find(|template| &template.name == name)
    }

    pub fn insert(&mut self, template: WorkspaceTemplate) -> Result<()> {
        template.validate()?;
        if let Some(existing) = self
            .templates
            .iter_mut()
            .find(|item| item.name == template.name)
        {
            *existing = template;
        } else {
            self.templates.push(template);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(command: &str) -> TemplateLayout {
        TemplateLayout::Pane(PaneTemplate {
            command: Some(command.into()),
            cwd: None,
            env: BTreeMap::new(),
            focus: false,
        })
    }

    #[test]
    fn nested_layout_validates_and_counts_panes() {
        let template = WorkspaceTemplate {
            name: TemplateName::try_from("review").unwrap(),
            tabs: vec![TabTemplate {
                name: Some("main".into()),
                layout: TemplateLayout::Split {
                    dir: SplitDir::Horizontal,
                    first: Box::new(pane("editor")),
                    second: Box::new(TemplateLayout::Split {
                        dir: SplitDir::Vertical,
                        first: Box::new(pane("tests")),
                        second: Box::new(pane("shell")),
                    }),
                },
                active: true,
            }],
        };
        template.validate().unwrap();
        assert_eq!(template.pane_count(), 3);
    }

    #[test]
    fn registry_replaces_by_typed_name_and_rejects_empty_template() {
        let name = TemplateName::try_from("default").unwrap();
        let base = WorkspaceTemplate {
            name: name.clone(),
            tabs: vec![TabTemplate {
                name: None,
                layout: pane("shell"),
                active: true,
            }],
        };
        let mut registry = TemplateRegistry::new(vec![base.clone()]).unwrap();
        let replacement = WorkspaceTemplate {
            name,
            tabs: vec![TabTemplate {
                name: None,
                layout: pane("editor"),
                active: true,
            }],
        };
        registry.insert(replacement).unwrap();
        assert_eq!(registry.templates().len(), 1);
        assert_eq!(registry.templates()[0].pane_count(), 1);

        let empty = WorkspaceTemplate {
            name: TemplateName::try_from("empty").unwrap(),
            tabs: Vec::new(),
        };
        assert!(TemplateRegistry::new(vec![empty]).is_err());
    }
}
