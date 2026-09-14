use crate::{
    config::FeaturesConfig,
    policy::{Policy, Risk},
};
use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Ready,
    OptionalDisabled,
}

impl ToolStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "pronto",
            Self::OptionalDisabled => "opcional/desabilitado",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub risk: Risk,
    pub feature: String,
    pub enabled: bool,
}

impl ToolSpec {
    pub fn status(&self) -> ToolStatus {
        if self.enabled {
            ToolStatus::Ready
        } else {
            ToolStatus::OptionalDisabled
        }
    }

    pub fn risk_name(&self) -> &'static str {
        match self.risk {
            Risk::Read => "read",
            Risk::ExternalWrite => "external_write",
            Risk::Destructive => "destructive",
            Risk::SecretInput => "secret_input",
        }
    }
}

pub struct ToolCatalog {
    tools: Vec<ToolSpec>,
}

impl ToolCatalog {
    /// Compatibility constructor for callers that only need the safe MVP.
    pub fn mvp(policy: &Policy) -> Self {
        let features = FeaturesConfig {
            memory: true,
            ..FeaturesConfig::default()
        };
        Self::for_features(policy, &features)
    }

    /// Builds the catalog from feature flags without starting a tool process.
    /// Discovery is intentionally separate from execution so disabled or
    /// untrusted capabilities cannot be reached accidentally.
    pub fn for_features(_policy: &Policy, features: &FeaturesConfig) -> Self {
        let mut tools = Vec::new();
        add_feature_tools(
            &mut tools,
            "memory",
            features.memory,
            [
                ("memory.search", "Search session memory", Risk::Read),
                ("memory.list", "List scoped memory records", Risk::Read),
                (
                    "memory.export",
                    "Export memory to a workspace file",
                    Risk::Read,
                ),
                (
                    "memory.delete",
                    "Delete memory for one session",
                    Risk::Destructive,
                ),
                ("memory.clear", "Delete all local memory", Risk::Destructive),
            ],
        );
        add_feature_tools(
            &mut tools,
            "browser",
            features.browser,
            [
                (
                    "browser.snapshot",
                    "Read the current browser accessibility snapshot",
                    Risk::Read,
                ),
                (
                    "browser.navigate",
                    "Navigate an isolated browser session",
                    Risk::ExternalWrite,
                ),
                (
                    "browser.interact",
                    "Interact with an isolated browser session",
                    Risk::ExternalWrite,
                ),
            ],
        );
        add_feature_tools(
            &mut tools,
            "computer_use",
            features.computer_use,
            [
                (
                    "computer.screenshot",
                    "Capture a desktop screenshot",
                    Risk::Read,
                ),
                (
                    "computer.action",
                    "Execute an approved desktop action",
                    Risk::ExternalWrite,
                ),
            ],
        );
        add_feature_tools(
            &mut tools,
            "shell",
            features.shell,
            [(
                "shell.exec",
                "Run an allowlisted workspace command",
                Risk::ExternalWrite,
            )],
        );
        add_feature_tools(
            &mut tools,
            "mcp",
            features.mcp,
            [
                ("mcp.list", "Discover allowlisted MCP tools", Risk::Read),
                (
                    "mcp.call",
                    "Call an allowlisted MCP tool",
                    Risk::ExternalWrite,
                ),
            ],
        );
        add_feature_tools(
            &mut tools,
            "channels",
            features.channels,
            [(
                "channel.send",
                "Send a message through an enabled channel",
                Risk::ExternalWrite,
            )],
        );
        add_feature_tools(
            &mut tools,
            "scheduler",
            features.scheduler,
            [(
                "schedule.manage",
                "Create, pause or remove scheduled jobs",
                Risk::ExternalWrite,
            )],
        );
        Self { tools }
    }

    pub fn list(&self) -> &[ToolSpec] {
        &self.tools
    }

    /// Discovers one tool on demand and applies the central policy gate.
    pub fn discover(&self, name: &str, policy: &Policy) -> Result<&ToolSpec> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .ok_or_else(|| anyhow::anyhow!("tool not found: {name}"))?;
        if !tool.enabled {
            anyhow::bail!(
                "tool {} is optional/desabilitado; habilite a feature {}",
                tool.name,
                tool.feature
            );
        }
        if policy.requires_approval(tool.risk) {
            anyhow::bail!("approval required for tool {}", tool.name);
        }
        Ok(tool)
    }
}

fn add_feature_tools<const N: usize>(
    tools: &mut Vec<ToolSpec>,
    feature: &str,
    enabled: bool,
    definitions: [(&str, &str, Risk); N],
) {
    tools.extend(
        definitions
            .into_iter()
            .map(|(name, description, risk)| ToolSpec {
                name: name.into(),
                description: description.into(),
                risk,
                feature: feature.into(),
                enabled,
            }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn catalog_exposes_ready_and_disabled_typed_tools() {
        let policy = Policy::supervised(PathBuf::from("."));
        let features = FeaturesConfig {
            memory: true,
            ..FeaturesConfig::default()
        };
        let catalog = ToolCatalog::for_features(&policy, &features);
        let search = catalog
            .list()
            .iter()
            .find(|tool| tool.name == "memory.search")
            .expect("memory tool");
        assert_eq!(search.status(), ToolStatus::Ready);
        assert_eq!(search.risk_name(), "read");
        let shell = catalog
            .list()
            .iter()
            .find(|tool| tool.name == "shell.exec")
            .expect("shell tool");
        assert_eq!(shell.status(), ToolStatus::OptionalDisabled);
    }

    #[test]
    fn discovery_is_on_demand_and_policy_gated() {
        let policy = Policy::supervised(PathBuf::from("."));
        let features = FeaturesConfig {
            memory: true,
            shell: true,
            ..FeaturesConfig::default()
        };
        let catalog = ToolCatalog::for_features(&policy, &features);
        assert!(catalog.discover("memory.search", &policy).is_ok());
        let error = catalog
            .discover("shell.exec", &policy)
            .expect_err("write tool must require approval");
        assert!(error.to_string().contains("approval required"));
        assert!(catalog.discover("browser.snapshot", &policy).is_err());
        assert!(catalog.discover("missing", &policy).is_err());
    }

    #[test]
    fn trusted_mode_allows_enabled_non_destructive_discovery() {
        let mut policy = Policy::supervised(PathBuf::from("."));
        policy.mode = "trusted".into();
        let features = FeaturesConfig {
            channels: true,
            ..FeaturesConfig::default()
        };
        let catalog = ToolCatalog::for_features(&policy, &features);
        assert_eq!(
            catalog
                .discover("channel.send", &policy)
                .expect("tool")
                .status(),
            ToolStatus::Ready
        );
    }
}
