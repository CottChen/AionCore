use std::collections::HashMap;

use aionui_common::TimestampMs;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// A. Permissions & Risk
// ---------------------------------------------------------------------------

/// Network access permission — either unrestricted (`true`) or domain-scoped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum NetworkPermission {
    /// Unrestricted network access (dangerous).
    Unrestricted(bool),
    /// Domain-scoped network access (moderate).
    Scoped {
        #[serde(rename = "allowedDomains")]
        allowed_domains: Vec<String>,
        reasoning: String,
    },
}

/// Filesystem access scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FilesystemScope {
    ExtensionOnly,
    Workspace,
    Full,
}

/// Extension permission declarations.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtPermissions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkPermission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem: Option<FilesystemScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipboard: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_user: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<bool>,
}

/// Overall risk level derived from permission declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RiskLevel {
    Safe,
    Moderate,
    Dangerous,
}

/// Granularity of a single permission entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionLevel {
    None,
    Limited,
    Full,
}

/// A single permission detail for display purposes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionDetail {
    pub permission: String,
    pub level: PermissionLevel,
    pub description: String,
}

/// Complete permission analysis summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSummary {
    pub permissions: ExtPermissions,
    pub risk_level: RiskLevel,
    pub details: Vec<PermissionDetail>,
}

// ---------------------------------------------------------------------------
// B. Contribution types (what an extension provides)
// ---------------------------------------------------------------------------

/// ACP adapter contributed by an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtAcpAdapter {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_cli_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acp_args: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_streaming: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yolo_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub api_key_fields: Vec<serde_json::Value>,
}

/// MCP server contributed by an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtMcpServer {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub config: serde_json::Value,
}

/// Assistant contributed by an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtAssistant {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Autonomous agent contributed by an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtAgent {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Skill contributed by an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtSkill {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Theme contributed by an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtTheme {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Relative path to the CSS file.
    pub css_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_image: Option<String>,
}

/// Channel plugin contributed by an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtChannelPlugin {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_point: Option<String>,
}

/// WebUI route definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtWebuiRoute {
    pub path: String,
    pub method: String,
    pub handler: String,
}

/// WebUI contribution from an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtWebui {
    pub id: String,
    pub directory: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<ExtWebuiRoute>,
}

/// Settings tab position relative to a built-in tab.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsTabPosition {
    pub relative_to: String,
    pub placement: String,
}

/// Settings tab contributed by an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtSettingsTab {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<SettingsTabPosition>,
}

/// Model provider contributed by an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtModelProvider {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

/// All contributions declared by an extension.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtContributes {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acp_adapters: Vec<ExtAcpAdapter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<ExtMcpServer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assistants: Vec<ExtAssistant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<ExtAgent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<ExtSkill>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub themes: Vec<ExtTheme>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_plugins: Vec<ExtChannelPlugin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub webui: Vec<ExtWebui>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings_tabs: Vec<ExtSettingsTab>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_providers: Vec<ExtModelProvider>,
}

// ---------------------------------------------------------------------------
// C. Extension manifest
// ---------------------------------------------------------------------------

/// i18n configuration block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct I18nConfig {
    pub locales: Vec<String>,
    #[serde(default = "default_i18n_directory")]
    pub directory: String,
}

fn default_i18n_directory() -> String {
    "i18n".to_owned()
}

/// Engine compatibility declaration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aionui: Option<String>,
}

/// Lifecycle hook declarations (paths relative to extension root).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleHooks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_install: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_uninstall: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_activate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_deactivate: Option<String>,
}

/// Complete extension manifest parsed from `aion-extension.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionManifest {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub dependencies: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_point: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<ExtPermissions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contributes: Option<ExtContributes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<LifecycleHooks>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i18n: Option<I18nConfig>,
}

// ---------------------------------------------------------------------------
// D. Extension runtime state
// ---------------------------------------------------------------------------

/// Where the extension was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtensionSource {
    Local,
    Appdata,
    Env,
}

/// Persisted state for an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionState {
    pub name: String,
    pub version: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<TimestampMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activated_at: Option<TimestampMs>,
}

/// A fully loaded extension with its manifest, location, and runtime state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoadedExtension {
    pub manifest: ExtensionManifest,
    pub directory: String,
    pub source: ExtensionSource,
    pub state: ExtensionState,
}

// ---------------------------------------------------------------------------
// E. Extension system events
// ---------------------------------------------------------------------------

/// Events emitted by the extension system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExtensionSystemEvent {
    ExtensionActivated,
    ExtensionDeactivated,
    ExtensionInstalled,
    ExtensionUninstalled,
    RegistryReloaded,
    StatesPersisted,
}

/// Payload for extension lifecycle events (M-46).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionLifecyclePayload {
    pub extension_name: String,
    pub event: ExtensionSystemEvent,
    pub timestamp: TimestampMs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// F. Hub types
// ---------------------------------------------------------------------------

/// Installation status of a Hub extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HubExtensionStatus {
    NotInstalled,
    Installed,
    UpdateAvailable,
    Installing,
    InstallFailed,
}

/// A Hub extension entry with runtime status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HubExtensionWithStatus {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub bundled: bool,
    pub status: HubExtensionStatus,
}

// ---------------------------------------------------------------------------
// G. Resolved contribution types (post-processing output)
// ---------------------------------------------------------------------------

/// Resolved WebUI contribution (after route validation).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WebuiContribution {
    pub extension_name: String,
    pub id: String,
    pub directory: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<ExtWebuiRoute>,
}

/// Resolved settings tab (after position parsing).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedSettingsTab {
    pub extension_name: String,
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<SettingsTabPosition>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- Permissions & Risk --

    #[test]
    fn test_risk_level_serde() {
        assert_eq!(serde_json::to_string(&RiskLevel::Safe).unwrap(), r#""safe""#);
        assert_eq!(
            serde_json::to_string(&RiskLevel::Moderate).unwrap(),
            r#""moderate""#
        );
        assert_eq!(
            serde_json::to_string(&RiskLevel::Dangerous).unwrap(),
            r#""dangerous""#
        );
    }

    #[test]
    fn test_network_permission_unrestricted() {
        let perm = NetworkPermission::Unrestricted(true);
        let json = serde_json::to_value(&perm).unwrap();
        assert_eq!(json, json!(true));
    }

    #[test]
    fn test_network_permission_scoped() {
        let perm = NetworkPermission::Scoped {
            allowed_domains: vec!["api.example.com".into()],
            reasoning: "needed for API calls".into(),
        };
        let json = serde_json::to_value(&perm).unwrap();
        assert_eq!(json["allowedDomains"], json!(["api.example.com"]));
        assert_eq!(json["reasoning"], "needed for API calls");
    }

    #[test]
    fn test_network_permission_scoped_deserialize() {
        let raw = json!({"allowedDomains": ["a.com"], "reasoning": "test"});
        let perm: NetworkPermission = serde_json::from_value(raw).unwrap();
        assert!(matches!(perm, NetworkPermission::Scoped { .. }));
    }

    #[test]
    fn test_filesystem_scope_serde() {
        assert_eq!(
            serde_json::to_string(&FilesystemScope::ExtensionOnly).unwrap(),
            r#""extension-only""#
        );
        assert_eq!(
            serde_json::to_string(&FilesystemScope::Workspace).unwrap(),
            r#""workspace""#
        );
        assert_eq!(
            serde_json::to_string(&FilesystemScope::Full).unwrap(),
            r#""full""#
        );
    }

    #[test]
    fn test_ext_permissions_empty() {
        let perms = ExtPermissions::default();
        let json = serde_json::to_value(&perms).unwrap();
        assert_eq!(json, json!({}));
    }

    #[test]
    fn test_ext_permissions_roundtrip() {
        let perms = ExtPermissions {
            storage: Some(true),
            network: Some(NetworkPermission::Unrestricted(true)),
            shell: Some(true),
            filesystem: Some(FilesystemScope::Full),
            clipboard: None,
            active_user: None,
            events: Some(true),
        };
        let json_str = serde_json::to_string(&perms).unwrap();
        let parsed: ExtPermissions = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, perms);
    }

    #[test]
    fn test_permission_level_serde() {
        let cases = [
            (PermissionLevel::None, r#""none""#),
            (PermissionLevel::Limited, r#""limited""#),
            (PermissionLevel::Full, r#""full""#),
        ];
        for (variant, expected) in cases {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
        }
    }

    // -- Contributions --

    #[test]
    fn test_ext_contributes_empty() {
        let c = ExtContributes::default();
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json, json!({}));
    }

    #[test]
    fn test_ext_contributes_with_skills() {
        let c = ExtContributes {
            skills: vec![ExtSkill {
                name: "my-skill".into(),
                description: Some("A test skill".into()),
                path: Some("skills/my-skill".into()),
            }],
            ..Default::default()
        };
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["skills"][0]["name"], "my-skill");
    }

    #[test]
    fn test_ext_acp_adapter_minimal() {
        let adapter = ExtAcpAdapter {
            id: "claude-adapter".into(),
            name: "Claude".into(),
            description: None,
            cli_command: Some("claude".into()),
            default_cli_path: None,
            acp_args: vec![],
            env: HashMap::new(),
            avatar: None,
            auth_required: None,
            supports_streaming: Some(true),
            connection_type: None,
            endpoint: None,
            models: vec![],
            yolo_mode: None,
            health_check: None,
            api_key_fields: vec![],
        };
        let json = serde_json::to_value(&adapter).unwrap();
        assert_eq!(json["id"], "claude-adapter");
        assert_eq!(json["cliCommand"], "claude");
        assert_eq!(json["supportsStreaming"], true);
        // Empty vecs should be omitted
        assert!(json.get("acpArgs").is_none());
    }

    #[test]
    fn test_ext_theme_serde() {
        let theme = ExtTheme {
            id: "dark".into(),
            name: "Dark Mode".into(),
            description: Some("A dark theme".into()),
            css_file: "themes/dark.css".into(),
            cover_image: Some("images/dark-preview.png".into()),
        };
        let json = serde_json::to_value(&theme).unwrap();
        assert_eq!(json["cssFile"], "themes/dark.css");
        assert_eq!(json["coverImage"], "images/dark-preview.png");
    }

    #[test]
    fn test_ext_webui_with_routes() {
        let webui = ExtWebui {
            id: "my-panel".into(),
            directory: "webui/dist".into(),
            routes: vec![ExtWebuiRoute {
                path: "/my-ext/api/data".into(),
                method: "GET".into(),
                handler: "handlers/data.js".into(),
            }],
        };
        let json = serde_json::to_value(&webui).unwrap();
        assert_eq!(json["routes"][0]["path"], "/my-ext/api/data");
        assert_eq!(json["routes"][0]["method"], "GET");
    }

    #[test]
    fn test_ext_settings_tab_with_position() {
        let tab = ExtSettingsTab {
            id: "ext-settings".into(),
            label: "Extension Settings".into(),
            icon: None,
            url: "aion-asset://ext/settings.html".into(),
            position: Some(SettingsTabPosition {
                relative_to: "general".into(),
                placement: "after".into(),
            }),
        };
        let json = serde_json::to_value(&tab).unwrap();
        assert_eq!(json["position"]["relativeTo"], "general");
        assert_eq!(json["position"]["placement"], "after");
    }

    // -- Manifest --

    #[test]
    fn test_manifest_minimal_deserialize() {
        let raw = json!({
            "name": "my-ext",
            "version": "1.0.0"
        });
        let manifest: ExtensionManifest = serde_json::from_value(raw).unwrap();
        assert_eq!(manifest.name, "my-ext");
        assert_eq!(manifest.version, "1.0.0");
        assert!(manifest.contributes.is_none());
        assert!(manifest.permissions.is_none());
        assert!(manifest.dependencies.is_empty());
    }

    #[test]
    fn test_manifest_full_roundtrip() {
        let manifest = ExtensionManifest {
            name: "test-ext".into(),
            version: "2.1.0".into(),
            display_name: Some("Test Extension".into()),
            description: Some("A test extension".into()),
            author: Some("Test Author".into()),
            license: Some("MIT".into()),
            homepage: Some("https://example.com".into()),
            icon: Some("icon.png".into()),
            engine: Some(EngineConfig {
                aionui: Some("^1.0.0".into()),
            }),
            api_version: Some("1.0.0".into()),
            dependencies: HashMap::from([("dep-ext".into(), "^1.0.0".into())]),
            entry_point: Some("main.js".into()),
            permissions: Some(ExtPermissions {
                storage: Some(true),
                events: Some(true),
                ..Default::default()
            }),
            contributes: Some(ExtContributes::default()),
            lifecycle: Some(LifecycleHooks {
                on_install: Some("scripts/install.sh".into()),
                on_activate: Some("scripts/activate.sh".into()),
                on_deactivate: None,
                on_uninstall: None,
            }),
            i18n: Some(I18nConfig {
                locales: vec!["en".into(), "zh-CN".into()],
                directory: "i18n".into(),
            }),
        };
        let json_str = serde_json::to_string(&manifest).unwrap();
        let parsed: ExtensionManifest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn test_manifest_camel_case_keys() {
        let manifest = ExtensionManifest {
            name: "x".into(),
            version: "1.0.0".into(),
            display_name: Some("X".into()),
            api_version: Some("1.0.0".into()),
            entry_point: Some("main.js".into()),
            description: None,
            author: None,
            license: None,
            homepage: None,
            icon: None,
            engine: None,
            dependencies: HashMap::new(),
            permissions: None,
            contributes: None,
            lifecycle: None,
            i18n: None,
        };
        let json = serde_json::to_value(&manifest).unwrap();
        assert!(json.get("displayName").is_some());
        assert!(json.get("apiVersion").is_some());
        assert!(json.get("entryPoint").is_some());
        // snake_case keys should not exist
        assert!(json.get("display_name").is_none());
        assert!(json.get("api_version").is_none());
    }

    // -- Extension state & source --

    #[test]
    fn test_extension_source_serde() {
        let cases = [
            (ExtensionSource::Local, r#""local""#),
            (ExtensionSource::Appdata, r#""appdata""#),
            (ExtensionSource::Env, r#""env""#),
        ];
        for (variant, expected) in cases {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
            let parsed: ExtensionSource = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant);
        }
    }

    #[test]
    fn test_extension_state_roundtrip() {
        let state = ExtensionState {
            name: "my-ext".into(),
            version: "1.0.0".into(),
            enabled: true,
            installed_at: Some(1700000000000),
            last_activated_at: Some(1700001000000),
        };
        let json_str = serde_json::to_string(&state).unwrap();
        let parsed: ExtensionState = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, state);
    }

    #[test]
    fn test_extension_state_optional_timestamps() {
        let raw = json!({
            "name": "x",
            "version": "1.0.0",
            "enabled": false
        });
        let state: ExtensionState = serde_json::from_value(raw).unwrap();
        assert!(!state.enabled);
        assert!(state.installed_at.is_none());
        assert!(state.last_activated_at.is_none());
    }

    // -- Events --

    #[test]
    fn test_extension_system_event_serde() {
        let cases = [
            (
                ExtensionSystemEvent::ExtensionActivated,
                r#""EXTENSION_ACTIVATED""#,
            ),
            (
                ExtensionSystemEvent::ExtensionDeactivated,
                r#""EXTENSION_DEACTIVATED""#,
            ),
            (
                ExtensionSystemEvent::ExtensionInstalled,
                r#""EXTENSION_INSTALLED""#,
            ),
            (
                ExtensionSystemEvent::ExtensionUninstalled,
                r#""EXTENSION_UNINSTALLED""#,
            ),
            (
                ExtensionSystemEvent::RegistryReloaded,
                r#""REGISTRY_RELOADED""#,
            ),
            (
                ExtensionSystemEvent::StatesPersisted,
                r#""STATES_PERSISTED""#,
            ),
        ];
        for (variant, expected) in cases {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
            let parsed: ExtensionSystemEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant);
        }
    }

    #[test]
    fn test_lifecycle_payload_roundtrip() {
        let payload = ExtensionLifecyclePayload {
            extension_name: "my-ext".into(),
            event: ExtensionSystemEvent::ExtensionActivated,
            timestamp: 1700000000000,
            data: Some(json!({"reason": "user action"})),
        };
        let json_str = serde_json::to_string(&payload).unwrap();
        let parsed: ExtensionLifecyclePayload = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, payload);
    }

    #[test]
    fn test_lifecycle_payload_without_data() {
        let payload = ExtensionLifecyclePayload {
            extension_name: "test".into(),
            event: ExtensionSystemEvent::RegistryReloaded,
            timestamp: 1700000000000,
            data: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert!(json.get("data").is_none());
    }

    // -- Hub --

    #[test]
    fn test_hub_extension_status_serde() {
        let cases = [
            (HubExtensionStatus::NotInstalled, r#""not_installed""#),
            (HubExtensionStatus::Installed, r#""installed""#),
            (HubExtensionStatus::UpdateAvailable, r#""update_available""#),
            (HubExtensionStatus::Installing, r#""installing""#),
            (HubExtensionStatus::InstallFailed, r#""install_failed""#),
        ];
        for (variant, expected) in cases {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
            let parsed: HubExtensionStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant);
        }
    }

    #[test]
    fn test_hub_extension_with_status_roundtrip() {
        let ext = HubExtensionWithStatus {
            name: "cool-ext".into(),
            version: "1.2.3".into(),
            display_name: Some("Cool Extension".into()),
            description: Some("Does cool things".into()),
            author: Some("Author".into()),
            icon: None,
            tags: vec!["productivity".into()],
            bundled: false,
            status: HubExtensionStatus::Installed,
        };
        let json_str = serde_json::to_string(&ext).unwrap();
        let parsed: HubExtensionWithStatus = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, ext);
    }

    #[test]
    fn test_hub_extension_bundled_status() {
        let ext = HubExtensionWithStatus {
            name: "builtin-ext".into(),
            version: "1.0.0".into(),
            display_name: None,
            description: None,
            author: None,
            icon: None,
            tags: vec![],
            bundled: true,
            status: HubExtensionStatus::Installed,
        };
        let json = serde_json::to_value(&ext).unwrap();
        assert_eq!(json["bundled"], true);
        assert_eq!(json["status"], "installed");
    }

    // -- Loaded extension --

    #[test]
    fn test_loaded_extension_roundtrip() {
        let loaded = LoadedExtension {
            manifest: ExtensionManifest {
                name: "test".into(),
                version: "1.0.0".into(),
                display_name: None,
                description: None,
                author: None,
                license: None,
                homepage: None,
                icon: None,
                engine: None,
                api_version: None,
                dependencies: HashMap::new(),
                entry_point: None,
                permissions: None,
                contributes: None,
                lifecycle: None,
                i18n: None,
            },
            directory: "/path/to/ext".into(),
            source: ExtensionSource::Env,
            state: ExtensionState {
                name: "test".into(),
                version: "1.0.0".into(),
                enabled: true,
                installed_at: None,
                last_activated_at: None,
            },
        };
        let json_str = serde_json::to_string(&loaded).unwrap();
        let parsed: LoadedExtension = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, loaded);
    }

    // -- I18n config --

    #[test]
    fn test_i18n_config_default_directory() {
        let raw = json!({"locales": ["en"]});
        let config: I18nConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(config.directory, "i18n");
    }

    #[test]
    fn test_i18n_config_custom_directory() {
        let raw = json!({"locales": ["en", "zh-CN"], "directory": "lang"});
        let config: I18nConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(config.directory, "lang");
    }

    // -- Lifecycle hooks --

    #[test]
    fn test_lifecycle_hooks_empty() {
        let hooks = LifecycleHooks::default();
        let json = serde_json::to_value(&hooks).unwrap();
        assert_eq!(json, json!({}));
    }

    #[test]
    fn test_lifecycle_hooks_partial() {
        let raw = json!({"onInstall": "scripts/install.sh"});
        let hooks: LifecycleHooks = serde_json::from_value(raw).unwrap();
        assert_eq!(hooks.on_install.as_deref(), Some("scripts/install.sh"));
        assert!(hooks.on_activate.is_none());
    }
}
