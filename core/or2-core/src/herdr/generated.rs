//! **Generated. Do not edit.** herdr's socket API types for herdr 0.9.3 (protocol 22),
//! produced by `scripts/gen-herdr-types.sh` from `schema.json` (the normalized output of
//! `herdr api schema --json`) with cargo-typify. To regenerate after a herdr update, run
//! `scripts/gen-herdr-types.sh`; fix problems in `scripts/herdr_schema.py`, never here.
//!
//! One module per schema family. Enums herdr sends carry a catch-all variant for values this
//! build does not know; unknown fields are ignored.

#![allow(clippy::all, dead_code, unused_imports)]

/// The herdr release this code was generated from.
pub const HERDR_VERSION: &str = "0.9.3";
/// The herdr API protocol number this code was generated from.
pub const PROTOCOL: u32 = 22;

/// Types generated from the `request` schema.
pub mod request {

    #[doc = "`AgentPromptParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentPromptParams {
        pub target: ::std::string::String,
        pub text: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub wait: ::std::option::Option<AgentPromptWaitOptions>,
    }
    #[doc = "`AgentPromptWaitOptions`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct AgentPromptWaitOptions {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub timeout_ms: ::std::option::Option<u64>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub until: ::std::vec::Vec<AgentStatus>,
    }
    #[doc = "`AgentReadParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentReadParams {
        #[serde(default = "defaults::agent_read_params_format")]
        pub format: ReadFormat,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub lines: ::std::option::Option<u32>,
        pub source: ReadSource,
        #[serde(default = "defaults::default_bool::<true>")]
        pub strip_ansi: bool,
        pub target: ::std::string::String,
    }
    #[doc = "`AgentRenameParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentRenameParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        pub target: ::std::string::String,
    }
    #[doc = "`AgentSendKeysParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentSendKeysParams {
        pub keys: ::std::vec::Vec<::std::string::String>,
        pub target: ::std::string::String,
    }
    #[doc = "`AgentStartParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentStartParams {
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub args: ::std::vec::Vec<::std::string::String>,
        pub kind: ::std::string::String,
        pub name: ::std::string::String,
        pub pane_id: ::std::string::String,
        #[doc = "Startup timeout in milliseconds. Values must be greater than 3000 and at most 300000."]
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub timeout_ms: ::std::option::Option<u64>,
    }
    #[doc = "`AgentStatus`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum AgentStatus {
        #[serde(rename = "idle")]
        Idle,
        #[serde(rename = "working")]
        Working,
        #[serde(rename = "blocked")]
        Blocked,
        #[serde(rename = "done")]
        Done,
        #[serde(rename = "unknown")]
        Unknown,
    }
    impl ::std::fmt::Display for AgentStatus {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Idle => f.write_str("idle"),
                Self::Working => f.write_str("working"),
                Self::Blocked => f.write_str("blocked"),
                Self::Done => f.write_str("done"),
                Self::Unknown => f.write_str("unknown"),
            }
        }
    }
    impl ::std::str::FromStr for AgentStatus {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "idle" => Ok(Self::Idle),
                "working" => Ok(Self::Working),
                "blocked" => Ok(Self::Blocked),
                "done" => Ok(Self::Done),
                "unknown" => Ok(Self::Unknown),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for AgentStatus {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for AgentStatus {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`AgentTarget`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentTarget {
        pub target: ::std::string::String,
    }
    #[doc = "`AgentViewBuiltinField`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum AgentViewBuiltinField {
        #[serde(rename = "status")]
        Status,
        #[serde(rename = "workspace_id")]
        WorkspaceId,
        #[serde(rename = "tab_id")]
        TabId,
        #[serde(rename = "pane_id")]
        PaneId,
        #[serde(rename = "agent")]
        Agent,
        #[serde(rename = "seen")]
        Seen,
        #[serde(rename = "state_change_seq")]
        StateChangeSeq,
    }
    impl ::std::fmt::Display for AgentViewBuiltinField {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Status => f.write_str("status"),
                Self::WorkspaceId => f.write_str("workspace_id"),
                Self::TabId => f.write_str("tab_id"),
                Self::PaneId => f.write_str("pane_id"),
                Self::Agent => f.write_str("agent"),
                Self::Seen => f.write_str("seen"),
                Self::StateChangeSeq => f.write_str("state_change_seq"),
            }
        }
    }
    impl ::std::str::FromStr for AgentViewBuiltinField {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "status" => Ok(Self::Status),
                "workspace_id" => Ok(Self::WorkspaceId),
                "tab_id" => Ok(Self::TabId),
                "pane_id" => Ok(Self::PaneId),
                "agent" => Ok(Self::Agent),
                "seen" => Ok(Self::Seen),
                "state_change_seq" => Ok(Self::StateChangeSeq),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for AgentViewBuiltinField {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for AgentViewBuiltinField {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`AgentViewBuiltinSortField`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum AgentViewBuiltinSortField {
        #[serde(rename = "workspace_order")]
        WorkspaceOrder,
        #[serde(rename = "tab_order")]
        TabOrder,
        #[serde(rename = "pane_order")]
        PaneOrder,
        #[serde(rename = "attention")]
        Attention,
        #[serde(rename = "status")]
        Status,
        #[serde(rename = "agent")]
        Agent,
        #[serde(rename = "seen")]
        Seen,
        #[serde(rename = "state_change_seq")]
        StateChangeSeq,
    }
    impl ::std::fmt::Display for AgentViewBuiltinSortField {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::WorkspaceOrder => f.write_str("workspace_order"),
                Self::TabOrder => f.write_str("tab_order"),
                Self::PaneOrder => f.write_str("pane_order"),
                Self::Attention => f.write_str("attention"),
                Self::Status => f.write_str("status"),
                Self::Agent => f.write_str("agent"),
                Self::Seen => f.write_str("seen"),
                Self::StateChangeSeq => f.write_str("state_change_seq"),
            }
        }
    }
    impl ::std::str::FromStr for AgentViewBuiltinSortField {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "workspace_order" => Ok(Self::WorkspaceOrder),
                "tab_order" => Ok(Self::TabOrder),
                "pane_order" => Ok(Self::PaneOrder),
                "attention" => Ok(Self::Attention),
                "status" => Ok(Self::Status),
                "agent" => Ok(Self::Agent),
                "seen" => Ok(Self::Seen),
                "state_change_seq" => Ok(Self::StateChangeSeq),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for AgentViewBuiltinSortField {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for AgentViewBuiltinSortField {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`AgentViewClearParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct AgentViewClearParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub source: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`AgentViewContext`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum AgentViewContext {
        #[serde(rename = "current_workspace_id")]
        CurrentWorkspaceId,
        #[serde(rename = "current_tab_id")]
        CurrentTabId,
    }
    impl ::std::fmt::Display for AgentViewContext {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::CurrentWorkspaceId => f.write_str("current_workspace_id"),
                Self::CurrentTabId => f.write_str("current_tab_id"),
            }
        }
    }
    impl ::std::str::FromStr for AgentViewContext {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "current_workspace_id" => Ok(Self::CurrentWorkspaceId),
                "current_tab_id" => Ok(Self::CurrentTabId),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for AgentViewContext {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for AgentViewContext {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`AgentViewField`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum AgentViewField {
        AgentViewBuiltinField(AgentViewBuiltinField),
        Object { token: ::std::string::String },
    }
    impl ::std::convert::From<AgentViewBuiltinField> for AgentViewField {
        fn from(value: AgentViewBuiltinField) -> Self {
            Self::AgentViewBuiltinField(value)
        }
    }
    #[doc = "`AgentViewFilter`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "op")]
    pub enum AgentViewFilter {
        #[serde(rename = "all")]
        All {
            filters: ::std::vec::Vec<AgentViewFilter>,
        },
        #[serde(rename = "any")]
        Any {
            filters: ::std::vec::Vec<AgentViewFilter>,
        },
        #[serde(rename = "not")]
        Not {
            filter: ::std::boxed::Box<AgentViewFilter>,
        },
        #[serde(rename = "eq")]
        Eq {
            field: AgentViewField,
            value: AgentViewValue,
        },
        #[serde(rename = "in")]
        In {
            field: AgentViewField,
            values: ::std::vec::Vec<AgentViewValue>,
        },
        #[serde(rename = "exists")]
        Exists { field: AgentViewField },
    }
    #[doc = "`AgentViewSetParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentViewSetParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub filter: ::std::option::Option<AgentViewFilter>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub label: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub sort: ::std::vec::Vec<AgentViewSort>,
        pub source: ::std::string::String,
    }
    #[doc = "`AgentViewSort`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentViewSort {
        pub field: AgentViewSortField,
        #[serde(default = "defaults::agent_view_sort_order")]
        pub order: AgentViewSortOrder,
    }
    #[doc = "`AgentViewSortField`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum AgentViewSortField {
        AgentViewBuiltinSortField(AgentViewBuiltinSortField),
        Object { token: ::std::string::String },
    }
    impl ::std::convert::From<AgentViewBuiltinSortField> for AgentViewSortField {
        fn from(value: AgentViewBuiltinSortField) -> Self {
            Self::AgentViewBuiltinSortField(value)
        }
    }
    #[doc = "`AgentViewSortOrder`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum AgentViewSortOrder {
        #[serde(rename = "asc")]
        Asc,
        #[serde(rename = "desc")]
        Desc,
    }
    impl ::std::fmt::Display for AgentViewSortOrder {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Asc => f.write_str("asc"),
                Self::Desc => f.write_str("desc"),
            }
        }
    }
    impl ::std::str::FromStr for AgentViewSortOrder {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "asc" => Ok(Self::Asc),
                "desc" => Ok(Self::Desc),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for AgentViewSortOrder {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for AgentViewSortOrder {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`AgentViewValue`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum AgentViewValue {
        String(::std::string::String),
        Boolean(bool),
        Uint64(u64),
        Object { context: AgentViewContext },
    }
    impl ::std::convert::From<bool> for AgentViewValue {
        fn from(value: bool) -> Self {
            Self::Boolean(value)
        }
    }
    impl ::std::convert::From<u64> for AgentViewValue {
        fn from(value: u64) -> Self {
            Self::Uint64(value)
        }
    }
    #[doc = "`AgentWaitParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentWaitParams {
        pub target: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub timeout_ms: ::std::option::Option<u64>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub until: ::std::vec::Vec<AgentStatus>,
    }
    #[doc = "Updates whether the requesting client shell receives and controls pane presentation."]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ClientShellSurfaceSetParams {
        pub active: bool,
    }
    #[doc = "`ClientWindowTitleSetParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ClientWindowTitleSetParams {
        pub title: ::std::string::String,
    }
    #[doc = "`CommandInvokeParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct CommandInvokeParams {
        #[doc = "Opaque endpoint-issued command identifier from the client-shell projection."]
        pub command_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
        #[doc = "Client-owned selection coordinates, validated against the pane's content revision."]
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub selection: ::std::option::Option<PaneSelectionReadParams>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub tab_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`EmptyParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(transparent)]
    pub struct EmptyParams(pub ::serde_json::Map<::std::string::String, ::serde_json::Value>);
    impl ::std::ops::Deref for EmptyParams {
        type Target = ::serde_json::Map<::std::string::String, ::serde_json::Value>;
        fn deref(&self) -> &::serde_json::Map<::std::string::String, ::serde_json::Value> {
            &self.0
        }
    }
    impl ::std::convert::From<EmptyParams>
        for ::serde_json::Map<::std::string::String, ::serde_json::Value>
    {
        fn from(value: EmptyParams) -> Self {
            value.0
        }
    }
    impl ::std::convert::From<::serde_json::Map<::std::string::String, ::serde_json::Value>>
        for EmptyParams
    {
        fn from(value: ::serde_json::Map<::std::string::String, ::serde_json::Value>) -> Self {
            Self(value)
        }
    }
    #[doc = "`EventMatch`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "event")]
    pub enum EventMatch {
        #[serde(rename = "workspace_created")]
        WorkspaceCreated {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            workspace_id: ::std::option::Option<::std::string::String>,
        },
        #[serde(rename = "workspace_updated")]
        WorkspaceUpdated { workspace_id: ::std::string::String },
        #[serde(rename = "workspace_closed")]
        WorkspaceClosed { workspace_id: ::std::string::String },
        #[serde(rename = "workspace_renamed")]
        WorkspaceRenamed {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            label: ::std::option::Option<::std::string::String>,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "workspace_moved")]
        WorkspaceMoved { workspace_id: ::std::string::String },
        #[serde(rename = "workspace_focused")]
        WorkspaceFocused { workspace_id: ::std::string::String },
        #[serde(rename = "tab_created")]
        TabCreated {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            tab_id: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            workspace_id: ::std::option::Option<::std::string::String>,
        },
        #[serde(rename = "tab_closed")]
        TabClosed { tab_id: ::std::string::String },
        #[serde(rename = "tab_renamed")]
        TabRenamed {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            label: ::std::option::Option<::std::string::String>,
            tab_id: ::std::string::String,
        },
        #[serde(rename = "tab_moved")]
        TabMoved { tab_id: ::std::string::String },
        #[serde(rename = "tab_focused")]
        TabFocused { tab_id: ::std::string::String },
        #[serde(rename = "pane_created")]
        PaneCreated {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            pane_id: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            workspace_id: ::std::option::Option<::std::string::String>,
        },
        #[serde(rename = "pane_closed")]
        PaneClosed { pane_id: ::std::string::String },
        #[serde(rename = "pane_focused")]
        PaneFocused { pane_id: ::std::string::String },
        #[serde(rename = "pane_moved")]
        PaneMoved { pane_id: ::std::string::String },
        #[serde(rename = "pane_output_changed")]
        PaneOutputChanged {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            min_revision: ::std::option::Option<u64>,
            pane_id: ::std::string::String,
        },
        #[serde(rename = "pane_exited")]
        PaneExited { pane_id: ::std::string::String },
        #[serde(rename = "pane_agent_detected")]
        PaneAgentDetected {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            agent: ::std::option::Option<::std::string::String>,
            pane_id: ::std::string::String,
        },
        #[serde(rename = "pane_agent_status_changed")]
        PaneAgentStatusChanged {
            agent_status: AgentStatus,
            pane_id: ::std::string::String,
        },
    }
    #[doc = "`EventsSubscribeParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct EventsSubscribeParams {
        pub subscriptions: ::std::vec::Vec<Subscription>,
    }
    #[doc = "`EventsWaitParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct EventsWaitParams {
        pub match_event: EventMatch,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub timeout_ms: ::std::option::Option<u64>,
    }
    #[doc = "`IntegrationInstallParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct IntegrationInstallParams {
        pub target: IntegrationTarget,
    }
    #[doc = "`IntegrationTarget`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum IntegrationTarget {
        #[serde(rename = "pi")]
        Pi,
        #[serde(rename = "omp")]
        Omp,
        #[serde(rename = "claude")]
        Claude,
        #[serde(rename = "codex")]
        Codex,
        #[serde(rename = "copilot")]
        Copilot,
        #[serde(rename = "devin")]
        Devin,
        #[serde(rename = "droid")]
        Droid,
        #[serde(rename = "kimi")]
        Kimi,
        #[serde(rename = "opencode")]
        Opencode,
        #[serde(rename = "kilo")]
        Kilo,
        #[serde(rename = "hermes")]
        Hermes,
        #[serde(rename = "qodercli")]
        Qodercli,
        #[serde(rename = "qwen")]
        Qwen,
        #[serde(rename = "cursor")]
        Cursor,
        #[serde(rename = "mastracode")]
        Mastracode,
        #[serde(rename = "antigravity_cli")]
        AntigravityCli,
        #[serde(rename = "grok")]
        Grok,
    }
    impl ::std::fmt::Display for IntegrationTarget {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Pi => f.write_str("pi"),
                Self::Omp => f.write_str("omp"),
                Self::Claude => f.write_str("claude"),
                Self::Codex => f.write_str("codex"),
                Self::Copilot => f.write_str("copilot"),
                Self::Devin => f.write_str("devin"),
                Self::Droid => f.write_str("droid"),
                Self::Kimi => f.write_str("kimi"),
                Self::Opencode => f.write_str("opencode"),
                Self::Kilo => f.write_str("kilo"),
                Self::Hermes => f.write_str("hermes"),
                Self::Qodercli => f.write_str("qodercli"),
                Self::Qwen => f.write_str("qwen"),
                Self::Cursor => f.write_str("cursor"),
                Self::Mastracode => f.write_str("mastracode"),
                Self::AntigravityCli => f.write_str("antigravity_cli"),
                Self::Grok => f.write_str("grok"),
            }
        }
    }
    impl ::std::str::FromStr for IntegrationTarget {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "pi" => Ok(Self::Pi),
                "omp" => Ok(Self::Omp),
                "claude" => Ok(Self::Claude),
                "codex" => Ok(Self::Codex),
                "copilot" => Ok(Self::Copilot),
                "devin" => Ok(Self::Devin),
                "droid" => Ok(Self::Droid),
                "kimi" => Ok(Self::Kimi),
                "opencode" => Ok(Self::Opencode),
                "kilo" => Ok(Self::Kilo),
                "hermes" => Ok(Self::Hermes),
                "qodercli" => Ok(Self::Qodercli),
                "qwen" => Ok(Self::Qwen),
                "cursor" => Ok(Self::Cursor),
                "mastracode" => Ok(Self::Mastracode),
                "antigravity_cli" => Ok(Self::AntigravityCli),
                "grok" => Ok(Self::Grok),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for IntegrationTarget {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for IntegrationTarget {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`IntegrationUninstallParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct IntegrationUninstallParams {
        pub target: IntegrationTarget,
    }
    #[doc = "`LayoutApplyParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct LayoutApplyParams {
        #[serde(default)]
        pub focus: bool,
        pub root: LayoutNode,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub tab_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub tab_label: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`LayoutExportParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct LayoutExportParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub tab_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`LayoutNode`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "type")]
    pub enum LayoutNode {
        #[serde(rename = "pane")]
        Pane {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            command: ::std::option::Option<::std::vec::Vec<::std::string::String>>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            cwd: ::std::option::Option<::std::string::String>,
            #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
            env: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            label: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            pane_id: ::std::option::Option<::std::string::String>,
        },
        #[serde(rename = "split")]
        Split {
            direction: SplitDirection,
            first: ::std::boxed::Box<LayoutNode>,
            ratio: f32,
            second: ::std::boxed::Box<LayoutNode>,
        },
    }
    #[doc = "`LayoutSetSplitRatioParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct LayoutSetSplitRatioParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
        pub path: ::std::vec::Vec<bool>,
        pub ratio: f32,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub tab_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`NotificationShowParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct NotificationShowParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub body: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub position: ::std::option::Option<ToastHerdrPosition>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub sound: ::std::option::Option<NotificationShowSound>,
        pub title: ::std::string::String,
    }
    #[doc = "`NotificationShowSound`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum NotificationShowSound {
        #[serde(rename = "none")]
        None,
        #[serde(rename = "done")]
        Done,
        #[serde(rename = "request")]
        Request,
    }
    impl ::std::fmt::Display for NotificationShowSound {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::None => f.write_str("none"),
                Self::Done => f.write_str("done"),
                Self::Request => f.write_str("request"),
            }
        }
    }
    impl ::std::str::FromStr for NotificationShowSound {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "none" => Ok(Self::None),
                "done" => Ok(Self::Done),
                "request" => Ok(Self::Request),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for NotificationShowSound {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for NotificationShowSound {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`OutputMatch`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "type", content = "value")]
    pub enum OutputMatch {
        #[serde(rename = "substring")]
        Substring(::std::string::String),
        #[serde(rename = "regex")]
        Regex(::std::string::String),
    }
    #[doc = "`PaneAgentState`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneAgentState {
        #[serde(rename = "idle")]
        Idle,
        #[serde(rename = "working")]
        Working,
        #[serde(rename = "blocked")]
        Blocked,
        #[serde(rename = "unknown")]
        Unknown,
    }
    impl ::std::fmt::Display for PaneAgentState {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Idle => f.write_str("idle"),
                Self::Working => f.write_str("working"),
                Self::Blocked => f.write_str("blocked"),
                Self::Unknown => f.write_str("unknown"),
            }
        }
    }
    impl ::std::str::FromStr for PaneAgentState {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "idle" => Ok(Self::Idle),
                "working" => Ok(Self::Working),
                "blocked" => Ok(Self::Blocked),
                "unknown" => Ok(Self::Unknown),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneAgentState {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneAgentState {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneClearAgentAuthorityParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneClearAgentAuthorityParams {
        pub pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub seq: ::std::option::Option<u64>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub source: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneCopyMotion`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneCopyMotion {
        #[serde(rename = "line_end")]
        LineEnd,
        #[serde(rename = "first_non_blank")]
        FirstNonBlank,
        #[serde(rename = "next_word_start")]
        NextWordStart,
        #[serde(rename = "previous_word_start")]
        PreviousWordStart,
        #[serde(rename = "next_word_end")]
        NextWordEnd,
        #[serde(rename = "next_big_word_start")]
        NextBigWordStart,
        #[serde(rename = "previous_big_word_start")]
        PreviousBigWordStart,
        #[serde(rename = "next_big_word_end")]
        NextBigWordEnd,
        #[serde(rename = "previous_paragraph")]
        PreviousParagraph,
        #[serde(rename = "next_paragraph")]
        NextParagraph,
    }
    impl ::std::fmt::Display for PaneCopyMotion {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::LineEnd => f.write_str("line_end"),
                Self::FirstNonBlank => f.write_str("first_non_blank"),
                Self::NextWordStart => f.write_str("next_word_start"),
                Self::PreviousWordStart => f.write_str("previous_word_start"),
                Self::NextWordEnd => f.write_str("next_word_end"),
                Self::NextBigWordStart => f.write_str("next_big_word_start"),
                Self::PreviousBigWordStart => f.write_str("previous_big_word_start"),
                Self::NextBigWordEnd => f.write_str("next_big_word_end"),
                Self::PreviousParagraph => f.write_str("previous_paragraph"),
                Self::NextParagraph => f.write_str("next_paragraph"),
            }
        }
    }
    impl ::std::str::FromStr for PaneCopyMotion {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "line_end" => Ok(Self::LineEnd),
                "first_non_blank" => Ok(Self::FirstNonBlank),
                "next_word_start" => Ok(Self::NextWordStart),
                "previous_word_start" => Ok(Self::PreviousWordStart),
                "next_word_end" => Ok(Self::NextWordEnd),
                "next_big_word_start" => Ok(Self::NextBigWordStart),
                "previous_big_word_start" => Ok(Self::PreviousBigWordStart),
                "next_big_word_end" => Ok(Self::NextBigWordEnd),
                "previous_paragraph" => Ok(Self::PreviousParagraph),
                "next_paragraph" => Ok(Self::NextParagraph),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneCopyMotion {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneCopyMotion {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneCopyMotionParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneCopyMotionParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub content_revision: ::std::option::Option<u64>,
        pub cursor: PaneTextPoint,
        pub motion: PaneCopyMotion,
        pub pane_id: ::std::string::String,
    }
    #[doc = "`PaneCopySearchDirection`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneCopySearchDirection {
        #[serde(rename = "forward")]
        Forward,
        #[serde(rename = "backward")]
        Backward,
    }
    impl ::std::fmt::Display for PaneCopySearchDirection {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Forward => f.write_str("forward"),
                Self::Backward => f.write_str("backward"),
            }
        }
    }
    impl ::std::str::FromStr for PaneCopySearchDirection {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "forward" => Ok(Self::Forward),
                "backward" => Ok(Self::Backward),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneCopySearchDirection {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneCopySearchDirection {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneCopySearchParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneCopySearchParams {
        pub content_revision: u64,
        pub cursor: PaneTextPoint,
        pub direction: PaneCopySearchDirection,
        pub pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub previous: ::std::option::Option<PaneTextRange>,
        pub query: ::std::string::String,
    }
    #[doc = "`PaneCurrentParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PaneCurrentParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub caller_pane_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneDirection`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneDirection {
        #[serde(rename = "left")]
        Left,
        #[serde(rename = "right")]
        Right,
        #[serde(rename = "up")]
        Up,
        #[serde(rename = "down")]
        Down,
    }
    impl ::std::fmt::Display for PaneDirection {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Left => f.write_str("left"),
                Self::Right => f.write_str("right"),
                Self::Up => f.write_str("up"),
                Self::Down => f.write_str("down"),
            }
        }
    }
    impl ::std::str::FromStr for PaneDirection {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "left" => Ok(Self::Left),
                "right" => Ok(Self::Right),
                "up" => Ok(Self::Up),
                "down" => Ok(Self::Down),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneDirection {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneDirection {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneEdgesParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PaneEdgesParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneFocusDirectionParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneFocusDirectionParams {
        pub direction: PaneDirection,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneInputSetParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneInputSetParams {
        pub pane_id: ::std::string::String,
        pub right_click: PaneRightClickTarget,
    }
    #[doc = "`PaneLayoutParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PaneLayoutParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneLinkActivateParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneLinkActivateParams {
        pub col: u16,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub content_revision: ::std::option::Option<u64>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub offset_from_bottom: ::std::option::Option<u64>,
        pub pane_id: ::std::string::String,
        pub viewport_row: u16,
    }
    #[doc = "`PaneListParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PaneListParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneMoveDestination`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "type")]
    pub enum PaneMoveDestination {
        #[serde(rename = "tab")]
        Tab {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            ratio: ::std::option::Option<f32>,
            split: SplitDirection,
            tab_id: ::std::string::String,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            target_pane_id: ::std::option::Option<::std::string::String>,
        },
        #[serde(rename = "new_tab")]
        NewTab {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            label: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            workspace_id: ::std::option::Option<::std::string::String>,
        },
        #[serde(rename = "new_workspace")]
        NewWorkspace {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            label: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            tab_label: ::std::option::Option<::std::string::String>,
        },
    }
    #[doc = "`PaneMoveParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneMoveParams {
        pub destination: PaneMoveDestination,
        #[serde(default)]
        pub focus: bool,
        pub pane_id: ::std::string::String,
    }
    #[doc = "`PaneNeighborParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneNeighborParams {
        pub direction: PaneDirection,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneProcessInfoParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PaneProcessInfoParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneReadParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneReadParams {
        #[serde(default = "defaults::pane_read_params_format")]
        pub format: ReadFormat,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub lines: ::std::option::Option<u32>,
        pub pane_id: ::std::string::String,
        pub source: ReadSource,
        #[serde(default = "defaults::default_bool::<true>")]
        pub strip_ansi: bool,
    }
    #[doc = "`PaneReleaseAgentParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneReleaseAgentParams {
        pub agent: ::std::string::String,
        pub pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub seq: ::std::option::Option<u64>,
        pub source: ::std::string::String,
    }
    #[doc = "`PaneRenameParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneRenameParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub label: ::std::option::Option<::std::string::String>,
        pub pane_id: ::std::string::String,
    }
    #[doc = "`PaneReportAgentParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneReportAgentParams {
        pub agent: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent_session_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent_session_path: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub message: ::std::option::Option<::std::string::String>,
        pub pane_id: ::std::string::String,
        #[doc = "Command that resumes this agent's session after a Herdr restart. The\nfirst element must be a plain command name."]
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub resume_argv: ::std::option::Option<::std::vec::Vec<::std::string::String>>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub seq: ::std::option::Option<u64>,
        pub source: ::std::string::String,
        pub state: PaneAgentState,
    }
    #[doc = "`PaneReportAgentSessionParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneReportAgentSessionParams {
        pub agent: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent_session_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent_session_path: ::std::option::Option<::std::string::String>,
        pub pane_id: ::std::string::String,
        #[doc = "Command that resumes this agent's session after a Herdr restart. The\nfirst element must be a plain command name."]
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub resume_argv: ::std::option::Option<::std::vec::Vec<::std::string::String>>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub seq: ::std::option::Option<u64>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub session_start_source: ::std::option::Option<::std::string::String>,
        pub source: ::std::string::String,
    }
    #[doc = "`PaneReportMetadataParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneReportMetadataParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub applies_to_source: ::std::option::Option<::std::string::String>,
        #[serde(default)]
        pub clear_display_agent: bool,
        #[serde(default)]
        pub clear_state_labels: bool,
        #[serde(default)]
        pub clear_title: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub display_agent: ::std::option::Option<::std::string::String>,
        pub pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub seq: ::std::option::Option<u64>,
        pub source: ::std::string::String,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub state_labels: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub title: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub tokens: ::std::collections::HashMap<
            ::std::string::String,
            ::std::option::Option<::std::string::String>,
        >,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub ttl_ms: ::std::option::Option<::std::num::NonZeroU64>,
    }
    #[doc = "`PaneResizeParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneResizeParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub amount: ::std::option::Option<f32>,
        pub direction: PaneDirection,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneRightClickTarget`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneRightClickTarget {
        #[serde(rename = "herdr")]
        Herdr,
        #[serde(rename = "pane")]
        Pane,
    }
    impl ::std::fmt::Display for PaneRightClickTarget {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Herdr => f.write_str("herdr"),
                Self::Pane => f.write_str("pane"),
            }
        }
    }
    impl ::std::str::FromStr for PaneRightClickTarget {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "herdr" => Ok(Self::Herdr),
                "pane" => Ok(Self::Pane),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneRightClickTarget {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneRightClickTarget {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneScrollParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneScrollParams {
        pub offset_from_bottom: u64,
        pub pane_id: ::std::string::String,
    }
    #[doc = "`PaneSelectionReadParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneSelectionReadParams {
        pub anchor: PaneTextPoint,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub content_revision: ::std::option::Option<u64>,
        pub cursor: PaneTextPoint,
        pub pane_id: ::std::string::String,
    }
    #[doc = "`PaneSendInputParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneSendInputParams {
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub keys: ::std::vec::Vec<::std::string::String>,
        pub pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub text: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneSendKeysParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneSendKeysParams {
        pub keys: ::std::vec::Vec<::std::string::String>,
        pub pane_id: ::std::string::String,
    }
    #[doc = "`PaneSendTextParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneSendTextParams {
        pub pane_id: ::std::string::String,
        pub text: ::std::string::String,
    }
    #[doc = "`PaneSplitParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneSplitParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        pub direction: SplitDirection,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub env: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        #[serde(default)]
        pub focus: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub ratio: ::std::option::Option<f32>,
        #[serde(default = "defaults::pane_split_params_right_click")]
        pub right_click: PaneRightClickTarget,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub target_pane_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneSwapParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PaneSwapParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub direction: ::std::option::Option<PaneDirection>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub source_pane_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub target_pane_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneTarget`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneTarget {
        pub pane_id: ::std::string::String,
    }
    #[doc = "`PaneTextPoint`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneTextPoint {
        pub col: u16,
        pub row: u32,
    }
    #[doc = "`PaneTextRange`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneTextRange {
        pub end: PaneTextPoint,
        pub start: PaneTextPoint,
    }
    #[doc = "`PaneWaitForOutputParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneWaitForOutputParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub lines: ::std::option::Option<u32>,
        #[serde(rename = "match")]
        pub match_: OutputMatch,
        pub pane_id: ::std::string::String,
        pub source: ReadSource,
        #[serde(default = "defaults::default_bool::<true>")]
        pub strip_ansi: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub timeout_ms: ::std::option::Option<u64>,
    }
    #[doc = "`PaneZoomMode`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneZoomMode {
        #[serde(rename = "toggle")]
        Toggle,
        #[serde(rename = "on")]
        On,
        #[serde(rename = "off")]
        Off,
    }
    impl ::std::fmt::Display for PaneZoomMode {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Toggle => f.write_str("toggle"),
                Self::On => f.write_str("on"),
                Self::Off => f.write_str("off"),
            }
        }
    }
    impl ::std::str::FromStr for PaneZoomMode {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "toggle" => Ok(Self::Toggle),
                "on" => Ok(Self::On),
                "off" => Ok(Self::Off),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneZoomMode {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneZoomMode {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneZoomParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneZoomParams {
        #[serde(default = "defaults::pane_zoom_params_mode")]
        pub mode: PaneZoomMode,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub pane_id: ::std::option::Option<::std::string::String>,
    }
    impl ::std::default::Default for PaneZoomParams {
        fn default() -> Self {
            Self {
                mode: defaults::pane_zoom_params_mode(),
                pane_id: Default::default(),
            }
        }
    }
    #[doc = "`PingParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(transparent)]
    pub struct PingParams(pub ::serde_json::Map<::std::string::String, ::serde_json::Value>);
    impl ::std::ops::Deref for PingParams {
        type Target = ::serde_json::Map<::std::string::String, ::serde_json::Value>;
        fn deref(&self) -> &::serde_json::Map<::std::string::String, ::serde_json::Value> {
            &self.0
        }
    }
    impl ::std::convert::From<PingParams>
        for ::serde_json::Map<::std::string::String, ::serde_json::Value>
    {
        fn from(value: PingParams) -> Self {
            value.0
        }
    }
    impl ::std::convert::From<::serde_json::Map<::std::string::String, ::serde_json::Value>>
        for PingParams
    {
        fn from(value: ::serde_json::Map<::std::string::String, ::serde_json::Value>) -> Self {
            Self(value)
        }
    }
    #[doc = "`PluginActionInvokeParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginActionInvokeParams {
        pub action_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub context: ::std::option::Option<PluginInvocationContext>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub plugin_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PluginActionListParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PluginActionListParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub plugin_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PluginInvocationContext`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PluginInvocationContext {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub clicked_url: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub correlation_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_pane_agent: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_pane_cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_pane_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_pane_status: ::std::option::Option<AgentStatus>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub invocation_source: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub link_handler_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub selected_text: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub tab_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub tab_label: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_label: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub worktree: ::std::option::Option<WorkspaceWorktreeInfo>,
    }
    #[doc = "`PluginLinkParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginLinkParams {
        #[serde(default = "defaults::default_bool::<true>")]
        pub enabled: bool,
        pub path: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub source: ::std::option::Option<PluginSourceInfo>,
    }
    #[doc = "`PluginListParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PluginListParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub plugin_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PluginLogListParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PluginLogListParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub limit: ::std::option::Option<u32>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub plugin_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PluginPaneCloseParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginPaneCloseParams {
        pub pane_id: ::std::string::String,
    }
    #[doc = "`PluginPaneFocusParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginPaneFocusParams {
        pub pane_id: ::std::string::String,
    }
    #[doc = "`PluginPaneOpenParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginPaneOpenParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub direction: ::std::option::Option<SplitDirection>,
        pub entrypoint: ::std::string::String,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub env: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        #[serde(default)]
        pub focus: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub height: ::std::option::Option<PopupSize>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub placement: ::std::option::Option<PluginPanePlacement>,
        pub plugin_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub target_pane_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub width: ::std::option::Option<PopupSize>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PluginPanePlacement`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PluginPanePlacement {
        #[serde(rename = "overlay")]
        Overlay,
        #[serde(rename = "popup")]
        Popup,
        #[serde(rename = "split")]
        Split,
        #[serde(rename = "tab")]
        Tab,
        #[serde(rename = "zoomed")]
        Zoomed,
    }
    impl ::std::fmt::Display for PluginPanePlacement {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Overlay => f.write_str("overlay"),
                Self::Popup => f.write_str("popup"),
                Self::Split => f.write_str("split"),
                Self::Tab => f.write_str("tab"),
                Self::Zoomed => f.write_str("zoomed"),
            }
        }
    }
    impl ::std::str::FromStr for PluginPanePlacement {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "overlay" => Ok(Self::Overlay),
                "popup" => Ok(Self::Popup),
                "split" => Ok(Self::Split),
                "tab" => Ok(Self::Tab),
                "zoomed" => Ok(Self::Zoomed),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PluginPanePlacement {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PluginPanePlacement {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PluginSetEnabledParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginSetEnabledParams {
        pub plugin_id: ::std::string::String,
    }
    #[doc = "`PluginSourceInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginSourceInfo {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub installed_unix_ms: ::std::option::Option<u64>,
        #[serde(default = "defaults::plugin_source_info_kind")]
        pub kind: PluginSourceKind,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub managed_path: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub owner: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub repo: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub requested_ref: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub resolved_commit: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub subdir: ::std::option::Option<::std::string::String>,
    }
    impl ::std::default::Default for PluginSourceInfo {
        fn default() -> Self {
            Self {
                installed_unix_ms: Default::default(),
                kind: defaults::plugin_source_info_kind(),
                managed_path: Default::default(),
                owner: Default::default(),
                repo: Default::default(),
                requested_ref: Default::default(),
                resolved_commit: Default::default(),
                subdir: Default::default(),
            }
        }
    }
    #[doc = "`PluginSourceKind`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PluginSourceKind {
        #[serde(rename = "local")]
        Local,
        #[serde(rename = "github")]
        Github,
    }
    impl ::std::fmt::Display for PluginSourceKind {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Local => f.write_str("local"),
                Self::Github => f.write_str("github"),
            }
        }
    }
    impl ::std::str::FromStr for PluginSourceKind {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "local" => Ok(Self::Local),
                "github" => Ok(Self::Github),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PluginSourceKind {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PluginSourceKind {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PluginUnlinkParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginUnlinkParams {
        pub plugin_id: ::std::string::String,
    }
    #[doc = "`PopupSize`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PopupSize {
        Integer(u16),
        String(::std::string::String),
    }
    impl ::std::fmt::Display for PopupSize {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Integer(x) => x.fmt(f),
                Self::String(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<u16> for PopupSize {
        fn from(value: u16) -> Self {
            Self::Integer(value)
        }
    }
    #[doc = "`ProductAnnouncementDismissParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ProductAnnouncementDismissParams {
        pub id: ::std::string::String,
        pub version: ::std::string::String,
    }
    #[doc = "`ReadFormat`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum ReadFormat {
        #[serde(rename = "text")]
        Text,
        #[serde(rename = "ansi")]
        Ansi,
    }
    impl ::std::fmt::Display for ReadFormat {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Text => f.write_str("text"),
                Self::Ansi => f.write_str("ansi"),
            }
        }
    }
    impl ::std::str::FromStr for ReadFormat {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "text" => Ok(Self::Text),
                "ansi" => Ok(Self::Ansi),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for ReadFormat {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for ReadFormat {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`ReadSource`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum ReadSource {
        #[serde(rename = "visible")]
        Visible,
        #[serde(rename = "recent")]
        Recent,
        #[serde(rename = "recent_unwrapped")]
        RecentUnwrapped,
        #[serde(rename = "detection")]
        Detection,
    }
    impl ::std::fmt::Display for ReadSource {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Visible => f.write_str("visible"),
                Self::Recent => f.write_str("recent"),
                Self::RecentUnwrapped => f.write_str("recent_unwrapped"),
                Self::Detection => f.write_str("detection"),
            }
        }
    }
    impl ::std::str::FromStr for ReadSource {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "visible" => Ok(Self::Visible),
                "recent" => Ok(Self::Recent),
                "recent_unwrapped" => Ok(Self::RecentUnwrapped),
                "detection" => Ok(Self::Detection),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for ReadSource {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for ReadSource {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`ReleaseNotesDismissParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ReleaseNotesDismissParams {
        pub version: ::std::string::String,
    }
    #[doc = "`RequestBody`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "method", content = "params")]
    pub enum RequestBody {
        #[serde(rename = "ping")]
        Ping(PingParams),
        #[serde(rename = "server.stop")]
        ServerStop(EmptyParams),
        #[serde(rename = "server.live_handoff")]
        ServerLiveHandoff(ServerLiveHandoffParams),
        #[serde(rename = "server.reload_config")]
        ServerReloadConfig(EmptyParams),
        #[serde(rename = "server.ssh_agent.register")]
        ServerSshAgentRegister(ServerSshAgentRegisterParams),
        #[serde(rename = "server.agent_manifests")]
        ServerAgentManifests(EmptyParams),
        #[serde(rename = "server.reload_agent_manifests")]
        ServerReloadAgentManifests(EmptyParams),
        #[serde(rename = "notification.show")]
        NotificationShow(NotificationShowParams),
        #[serde(rename = "product_announcement.dismiss")]
        ProductAnnouncementDismiss(ProductAnnouncementDismissParams),
        #[serde(rename = "release_notes.dismiss")]
        ReleaseNotesDismiss(ReleaseNotesDismissParams),
        #[serde(rename = "command.invoke")]
        CommandInvoke(CommandInvokeParams),
        #[serde(rename = "client.window_title.set")]
        ClientWindowTitleSet(ClientWindowTitleSetParams),
        #[serde(rename = "client.window_title.clear")]
        ClientWindowTitleClear(EmptyParams),
        #[serde(rename = "client_shell.surface.set")]
        ClientShellSurfaceSet(ClientShellSurfaceSetParams),
        #[serde(rename = "session.snapshot")]
        SessionSnapshot(EmptyParams),
        #[serde(rename = "workspace.create")]
        WorkspaceCreate(WorkspaceCreateParams),
        #[serde(rename = "workspace.list")]
        WorkspaceList(EmptyParams),
        #[serde(rename = "workspace.get")]
        WorkspaceGet(WorkspaceTarget),
        #[serde(rename = "workspace.focus")]
        WorkspaceFocus(WorkspaceTarget),
        #[serde(rename = "workspace.rename")]
        WorkspaceRename(WorkspaceRenameParams),
        #[serde(rename = "workspace.move")]
        WorkspaceMove(WorkspaceMoveParams),
        #[serde(rename = "workspace.move_block")]
        WorkspaceMoveBlock(WorkspaceMoveBlockParams),
        #[serde(rename = "workspace.report_metadata")]
        WorkspaceReportMetadata(WorkspaceReportMetadataParams),
        #[serde(rename = "workspace.close")]
        WorkspaceClose(WorkspaceCloseParams),
        #[serde(rename = "worktree.list")]
        WorktreeList(WorktreeListParams),
        #[serde(rename = "worktree.create")]
        WorktreeCreate(WorktreeCreateParams),
        #[serde(rename = "worktree.open")]
        WorktreeOpen(WorktreeOpenParams),
        #[serde(rename = "worktree.remove")]
        WorktreeRemove(WorktreeRemoveParams),
        #[serde(rename = "tab.create")]
        TabCreate(TabCreateParams),
        #[serde(rename = "tab.list")]
        TabList(TabListParams),
        #[serde(rename = "tab.get")]
        TabGet(TabTarget),
        #[serde(rename = "tab.focus")]
        TabFocus(TabTarget),
        #[serde(rename = "tab.rename")]
        TabRename(TabRenameParams),
        #[serde(rename = "tab.move")]
        TabMove(TabMoveParams),
        #[serde(rename = "tab.close")]
        TabClose(TabTarget),
        #[serde(rename = "agent.list")]
        AgentList(EmptyParams),
        #[serde(rename = "agent.get")]
        AgentGet(AgentTarget),
        #[serde(rename = "agent.read")]
        AgentRead(AgentReadParams),
        #[serde(rename = "agent.explain")]
        AgentExplain(AgentTarget),
        #[serde(rename = "agent.send_keys")]
        AgentSendKeys(AgentSendKeysParams),
        #[serde(rename = "agent.rename")]
        AgentRename(AgentRenameParams),
        #[serde(rename = "agent.view.set")]
        AgentViewSet(AgentViewSetParams),
        #[serde(rename = "agent.view.clear")]
        AgentViewClear(AgentViewClearParams),
        #[serde(rename = "agent.focus")]
        AgentFocus(AgentTarget),
        #[serde(rename = "agent.start")]
        AgentStart(AgentStartParams),
        #[serde(rename = "agent.prompt")]
        AgentPrompt(AgentPromptParams),
        #[serde(rename = "agent.wait")]
        AgentWait(AgentWaitParams),
        #[serde(rename = "pane.split")]
        PaneSplit(PaneSplitParams),
        #[serde(rename = "pane.swap")]
        PaneSwap(PaneSwapParams),
        #[serde(rename = "pane.move")]
        PaneMove(PaneMoveParams),
        #[serde(rename = "pane.zoom")]
        PaneZoom(PaneZoomParams),
        #[serde(rename = "pane.layout")]
        PaneLayout(PaneLayoutParams),
        #[serde(rename = "pane.process_info")]
        PaneProcessInfo(PaneProcessInfoParams),
        #[serde(rename = "layout.export")]
        LayoutExport(LayoutExportParams),
        #[serde(rename = "layout.apply")]
        LayoutApply(LayoutApplyParams),
        #[serde(rename = "layout.set_split_ratio")]
        LayoutSetSplitRatio(LayoutSetSplitRatioParams),
        #[serde(rename = "pane.neighbor")]
        PaneNeighbor(PaneNeighborParams),
        #[serde(rename = "pane.edges")]
        PaneEdges(PaneEdgesParams),
        #[serde(rename = "pane.focus_direction")]
        PaneFocusDirection(PaneFocusDirectionParams),
        #[serde(rename = "pane.resize")]
        PaneResize(PaneResizeParams),
        #[serde(rename = "pane.scroll")]
        PaneScroll(PaneScrollParams),
        #[serde(rename = "pane.clear")]
        PaneClear(PaneTarget),
        #[serde(rename = "pane.edit_scrollback")]
        PaneEditScrollback(PaneTarget),
        #[serde(rename = "pane.selection.read")]
        PaneSelectionRead(PaneSelectionReadParams),
        #[serde(rename = "pane.copy_motion")]
        PaneCopyMotion(PaneCopyMotionParams),
        #[serde(rename = "pane.copy_search")]
        PaneCopySearch(PaneCopySearchParams),
        #[serde(rename = "pane.list")]
        PaneList(PaneListParams),
        #[serde(rename = "pane.current")]
        PaneCurrent(PaneCurrentParams),
        #[serde(rename = "pane.get")]
        PaneGet(PaneTarget),
        #[serde(rename = "pane.focus")]
        PaneFocus(PaneTarget),
        #[serde(rename = "pane.input.set")]
        PaneInputSet(PaneInputSetParams),
        #[serde(rename = "pane.link.activate")]
        PaneLinkActivate(PaneLinkActivateParams),
        #[serde(rename = "pane.link.resolve")]
        PaneLinkResolve(PaneLinkActivateParams),
        #[serde(rename = "pane.rename")]
        PaneRename(PaneRenameParams),
        #[serde(rename = "pane.send_text")]
        PaneSendText(PaneSendTextParams),
        #[serde(rename = "pane.send_keys")]
        PaneSendKeys(PaneSendKeysParams),
        #[serde(rename = "pane.send_input")]
        PaneSendInput(PaneSendInputParams),
        #[serde(rename = "pane.read")]
        PaneRead(PaneReadParams),
        #[serde(rename = "pane.report_agent")]
        PaneReportAgent(PaneReportAgentParams),
        #[serde(rename = "pane.report_agent_session")]
        PaneReportAgentSession(PaneReportAgentSessionParams),
        #[serde(rename = "pane.report_metadata")]
        PaneReportMetadata(PaneReportMetadataParams),
        #[serde(rename = "pane.clear_agent_authority")]
        PaneClearAgentAuthority(PaneClearAgentAuthorityParams),
        #[serde(rename = "pane.release_agent")]
        PaneReleaseAgent(PaneReleaseAgentParams),
        #[serde(rename = "pane.close")]
        PaneClose(PaneTarget),
        #[serde(rename = "popup.close")]
        PopupClose(EmptyParams),
        #[serde(rename = "events.subscribe")]
        EventsSubscribe(EventsSubscribeParams),
        #[serde(rename = "events.wait")]
        EventsWait(EventsWaitParams),
        #[serde(rename = "pane.wait_for_output")]
        PaneWaitForOutput(PaneWaitForOutputParams),
        #[serde(rename = "integration.list")]
        IntegrationList(EmptyParams),
        #[serde(rename = "integration.install")]
        IntegrationInstall(IntegrationInstallParams),
        #[serde(rename = "integration.uninstall")]
        IntegrationUninstall(IntegrationUninstallParams),
        #[serde(rename = "plugin.link")]
        PluginLink(PluginLinkParams),
        #[serde(rename = "plugin.list")]
        PluginList(PluginListParams),
        #[serde(rename = "plugin.unlink")]
        PluginUnlink(PluginUnlinkParams),
        #[serde(rename = "plugin.enable")]
        PluginEnable(PluginSetEnabledParams),
        #[serde(rename = "plugin.disable")]
        PluginDisable(PluginSetEnabledParams),
        #[serde(rename = "plugin.action.list")]
        PluginActionList(PluginActionListParams),
        #[serde(rename = "plugin.action.invoke")]
        PluginActionInvoke(PluginActionInvokeParams),
        #[serde(rename = "plugin.log.list")]
        PluginLogList(PluginLogListParams),
        #[serde(rename = "plugin.pane.open")]
        PluginPaneOpen(PluginPaneOpenParams),
        #[serde(rename = "plugin.pane.focus")]
        PluginPaneFocus(PluginPaneFocusParams),
        #[serde(rename = "plugin.pane.close")]
        PluginPaneClose(PluginPaneCloseParams),
    }
    impl ::std::convert::From<PingParams> for RequestBody {
        fn from(value: PingParams) -> Self {
            Self::Ping(value)
        }
    }
    impl ::std::convert::From<ServerLiveHandoffParams> for RequestBody {
        fn from(value: ServerLiveHandoffParams) -> Self {
            Self::ServerLiveHandoff(value)
        }
    }
    impl ::std::convert::From<ServerSshAgentRegisterParams> for RequestBody {
        fn from(value: ServerSshAgentRegisterParams) -> Self {
            Self::ServerSshAgentRegister(value)
        }
    }
    impl ::std::convert::From<NotificationShowParams> for RequestBody {
        fn from(value: NotificationShowParams) -> Self {
            Self::NotificationShow(value)
        }
    }
    impl ::std::convert::From<ProductAnnouncementDismissParams> for RequestBody {
        fn from(value: ProductAnnouncementDismissParams) -> Self {
            Self::ProductAnnouncementDismiss(value)
        }
    }
    impl ::std::convert::From<ReleaseNotesDismissParams> for RequestBody {
        fn from(value: ReleaseNotesDismissParams) -> Self {
            Self::ReleaseNotesDismiss(value)
        }
    }
    impl ::std::convert::From<CommandInvokeParams> for RequestBody {
        fn from(value: CommandInvokeParams) -> Self {
            Self::CommandInvoke(value)
        }
    }
    impl ::std::convert::From<ClientWindowTitleSetParams> for RequestBody {
        fn from(value: ClientWindowTitleSetParams) -> Self {
            Self::ClientWindowTitleSet(value)
        }
    }
    impl ::std::convert::From<ClientShellSurfaceSetParams> for RequestBody {
        fn from(value: ClientShellSurfaceSetParams) -> Self {
            Self::ClientShellSurfaceSet(value)
        }
    }
    impl ::std::convert::From<WorkspaceCreateParams> for RequestBody {
        fn from(value: WorkspaceCreateParams) -> Self {
            Self::WorkspaceCreate(value)
        }
    }
    impl ::std::convert::From<WorkspaceRenameParams> for RequestBody {
        fn from(value: WorkspaceRenameParams) -> Self {
            Self::WorkspaceRename(value)
        }
    }
    impl ::std::convert::From<WorkspaceMoveParams> for RequestBody {
        fn from(value: WorkspaceMoveParams) -> Self {
            Self::WorkspaceMove(value)
        }
    }
    impl ::std::convert::From<WorkspaceMoveBlockParams> for RequestBody {
        fn from(value: WorkspaceMoveBlockParams) -> Self {
            Self::WorkspaceMoveBlock(value)
        }
    }
    impl ::std::convert::From<WorkspaceReportMetadataParams> for RequestBody {
        fn from(value: WorkspaceReportMetadataParams) -> Self {
            Self::WorkspaceReportMetadata(value)
        }
    }
    impl ::std::convert::From<WorkspaceCloseParams> for RequestBody {
        fn from(value: WorkspaceCloseParams) -> Self {
            Self::WorkspaceClose(value)
        }
    }
    impl ::std::convert::From<WorktreeListParams> for RequestBody {
        fn from(value: WorktreeListParams) -> Self {
            Self::WorktreeList(value)
        }
    }
    impl ::std::convert::From<WorktreeCreateParams> for RequestBody {
        fn from(value: WorktreeCreateParams) -> Self {
            Self::WorktreeCreate(value)
        }
    }
    impl ::std::convert::From<WorktreeOpenParams> for RequestBody {
        fn from(value: WorktreeOpenParams) -> Self {
            Self::WorktreeOpen(value)
        }
    }
    impl ::std::convert::From<WorktreeRemoveParams> for RequestBody {
        fn from(value: WorktreeRemoveParams) -> Self {
            Self::WorktreeRemove(value)
        }
    }
    impl ::std::convert::From<TabCreateParams> for RequestBody {
        fn from(value: TabCreateParams) -> Self {
            Self::TabCreate(value)
        }
    }
    impl ::std::convert::From<TabListParams> for RequestBody {
        fn from(value: TabListParams) -> Self {
            Self::TabList(value)
        }
    }
    impl ::std::convert::From<TabRenameParams> for RequestBody {
        fn from(value: TabRenameParams) -> Self {
            Self::TabRename(value)
        }
    }
    impl ::std::convert::From<TabMoveParams> for RequestBody {
        fn from(value: TabMoveParams) -> Self {
            Self::TabMove(value)
        }
    }
    impl ::std::convert::From<AgentReadParams> for RequestBody {
        fn from(value: AgentReadParams) -> Self {
            Self::AgentRead(value)
        }
    }
    impl ::std::convert::From<AgentSendKeysParams> for RequestBody {
        fn from(value: AgentSendKeysParams) -> Self {
            Self::AgentSendKeys(value)
        }
    }
    impl ::std::convert::From<AgentRenameParams> for RequestBody {
        fn from(value: AgentRenameParams) -> Self {
            Self::AgentRename(value)
        }
    }
    impl ::std::convert::From<AgentViewSetParams> for RequestBody {
        fn from(value: AgentViewSetParams) -> Self {
            Self::AgentViewSet(value)
        }
    }
    impl ::std::convert::From<AgentViewClearParams> for RequestBody {
        fn from(value: AgentViewClearParams) -> Self {
            Self::AgentViewClear(value)
        }
    }
    impl ::std::convert::From<AgentStartParams> for RequestBody {
        fn from(value: AgentStartParams) -> Self {
            Self::AgentStart(value)
        }
    }
    impl ::std::convert::From<AgentPromptParams> for RequestBody {
        fn from(value: AgentPromptParams) -> Self {
            Self::AgentPrompt(value)
        }
    }
    impl ::std::convert::From<AgentWaitParams> for RequestBody {
        fn from(value: AgentWaitParams) -> Self {
            Self::AgentWait(value)
        }
    }
    impl ::std::convert::From<PaneSplitParams> for RequestBody {
        fn from(value: PaneSplitParams) -> Self {
            Self::PaneSplit(value)
        }
    }
    impl ::std::convert::From<PaneSwapParams> for RequestBody {
        fn from(value: PaneSwapParams) -> Self {
            Self::PaneSwap(value)
        }
    }
    impl ::std::convert::From<PaneMoveParams> for RequestBody {
        fn from(value: PaneMoveParams) -> Self {
            Self::PaneMove(value)
        }
    }
    impl ::std::convert::From<PaneZoomParams> for RequestBody {
        fn from(value: PaneZoomParams) -> Self {
            Self::PaneZoom(value)
        }
    }
    impl ::std::convert::From<PaneLayoutParams> for RequestBody {
        fn from(value: PaneLayoutParams) -> Self {
            Self::PaneLayout(value)
        }
    }
    impl ::std::convert::From<PaneProcessInfoParams> for RequestBody {
        fn from(value: PaneProcessInfoParams) -> Self {
            Self::PaneProcessInfo(value)
        }
    }
    impl ::std::convert::From<LayoutExportParams> for RequestBody {
        fn from(value: LayoutExportParams) -> Self {
            Self::LayoutExport(value)
        }
    }
    impl ::std::convert::From<LayoutApplyParams> for RequestBody {
        fn from(value: LayoutApplyParams) -> Self {
            Self::LayoutApply(value)
        }
    }
    impl ::std::convert::From<LayoutSetSplitRatioParams> for RequestBody {
        fn from(value: LayoutSetSplitRatioParams) -> Self {
            Self::LayoutSetSplitRatio(value)
        }
    }
    impl ::std::convert::From<PaneNeighborParams> for RequestBody {
        fn from(value: PaneNeighborParams) -> Self {
            Self::PaneNeighbor(value)
        }
    }
    impl ::std::convert::From<PaneEdgesParams> for RequestBody {
        fn from(value: PaneEdgesParams) -> Self {
            Self::PaneEdges(value)
        }
    }
    impl ::std::convert::From<PaneFocusDirectionParams> for RequestBody {
        fn from(value: PaneFocusDirectionParams) -> Self {
            Self::PaneFocusDirection(value)
        }
    }
    impl ::std::convert::From<PaneResizeParams> for RequestBody {
        fn from(value: PaneResizeParams) -> Self {
            Self::PaneResize(value)
        }
    }
    impl ::std::convert::From<PaneScrollParams> for RequestBody {
        fn from(value: PaneScrollParams) -> Self {
            Self::PaneScroll(value)
        }
    }
    impl ::std::convert::From<PaneSelectionReadParams> for RequestBody {
        fn from(value: PaneSelectionReadParams) -> Self {
            Self::PaneSelectionRead(value)
        }
    }
    impl ::std::convert::From<PaneCopyMotionParams> for RequestBody {
        fn from(value: PaneCopyMotionParams) -> Self {
            Self::PaneCopyMotion(value)
        }
    }
    impl ::std::convert::From<PaneCopySearchParams> for RequestBody {
        fn from(value: PaneCopySearchParams) -> Self {
            Self::PaneCopySearch(value)
        }
    }
    impl ::std::convert::From<PaneListParams> for RequestBody {
        fn from(value: PaneListParams) -> Self {
            Self::PaneList(value)
        }
    }
    impl ::std::convert::From<PaneCurrentParams> for RequestBody {
        fn from(value: PaneCurrentParams) -> Self {
            Self::PaneCurrent(value)
        }
    }
    impl ::std::convert::From<PaneInputSetParams> for RequestBody {
        fn from(value: PaneInputSetParams) -> Self {
            Self::PaneInputSet(value)
        }
    }
    impl ::std::convert::From<PaneRenameParams> for RequestBody {
        fn from(value: PaneRenameParams) -> Self {
            Self::PaneRename(value)
        }
    }
    impl ::std::convert::From<PaneSendTextParams> for RequestBody {
        fn from(value: PaneSendTextParams) -> Self {
            Self::PaneSendText(value)
        }
    }
    impl ::std::convert::From<PaneSendKeysParams> for RequestBody {
        fn from(value: PaneSendKeysParams) -> Self {
            Self::PaneSendKeys(value)
        }
    }
    impl ::std::convert::From<PaneSendInputParams> for RequestBody {
        fn from(value: PaneSendInputParams) -> Self {
            Self::PaneSendInput(value)
        }
    }
    impl ::std::convert::From<PaneReadParams> for RequestBody {
        fn from(value: PaneReadParams) -> Self {
            Self::PaneRead(value)
        }
    }
    impl ::std::convert::From<PaneReportAgentParams> for RequestBody {
        fn from(value: PaneReportAgentParams) -> Self {
            Self::PaneReportAgent(value)
        }
    }
    impl ::std::convert::From<PaneReportAgentSessionParams> for RequestBody {
        fn from(value: PaneReportAgentSessionParams) -> Self {
            Self::PaneReportAgentSession(value)
        }
    }
    impl ::std::convert::From<PaneReportMetadataParams> for RequestBody {
        fn from(value: PaneReportMetadataParams) -> Self {
            Self::PaneReportMetadata(value)
        }
    }
    impl ::std::convert::From<PaneClearAgentAuthorityParams> for RequestBody {
        fn from(value: PaneClearAgentAuthorityParams) -> Self {
            Self::PaneClearAgentAuthority(value)
        }
    }
    impl ::std::convert::From<PaneReleaseAgentParams> for RequestBody {
        fn from(value: PaneReleaseAgentParams) -> Self {
            Self::PaneReleaseAgent(value)
        }
    }
    impl ::std::convert::From<EventsSubscribeParams> for RequestBody {
        fn from(value: EventsSubscribeParams) -> Self {
            Self::EventsSubscribe(value)
        }
    }
    impl ::std::convert::From<EventsWaitParams> for RequestBody {
        fn from(value: EventsWaitParams) -> Self {
            Self::EventsWait(value)
        }
    }
    impl ::std::convert::From<PaneWaitForOutputParams> for RequestBody {
        fn from(value: PaneWaitForOutputParams) -> Self {
            Self::PaneWaitForOutput(value)
        }
    }
    impl ::std::convert::From<IntegrationInstallParams> for RequestBody {
        fn from(value: IntegrationInstallParams) -> Self {
            Self::IntegrationInstall(value)
        }
    }
    impl ::std::convert::From<IntegrationUninstallParams> for RequestBody {
        fn from(value: IntegrationUninstallParams) -> Self {
            Self::IntegrationUninstall(value)
        }
    }
    impl ::std::convert::From<PluginLinkParams> for RequestBody {
        fn from(value: PluginLinkParams) -> Self {
            Self::PluginLink(value)
        }
    }
    impl ::std::convert::From<PluginListParams> for RequestBody {
        fn from(value: PluginListParams) -> Self {
            Self::PluginList(value)
        }
    }
    impl ::std::convert::From<PluginUnlinkParams> for RequestBody {
        fn from(value: PluginUnlinkParams) -> Self {
            Self::PluginUnlink(value)
        }
    }
    impl ::std::convert::From<PluginActionListParams> for RequestBody {
        fn from(value: PluginActionListParams) -> Self {
            Self::PluginActionList(value)
        }
    }
    impl ::std::convert::From<PluginActionInvokeParams> for RequestBody {
        fn from(value: PluginActionInvokeParams) -> Self {
            Self::PluginActionInvoke(value)
        }
    }
    impl ::std::convert::From<PluginLogListParams> for RequestBody {
        fn from(value: PluginLogListParams) -> Self {
            Self::PluginLogList(value)
        }
    }
    impl ::std::convert::From<PluginPaneOpenParams> for RequestBody {
        fn from(value: PluginPaneOpenParams) -> Self {
            Self::PluginPaneOpen(value)
        }
    }
    impl ::std::convert::From<PluginPaneFocusParams> for RequestBody {
        fn from(value: PluginPaneFocusParams) -> Self {
            Self::PluginPaneFocus(value)
        }
    }
    impl ::std::convert::From<PluginPaneCloseParams> for RequestBody {
        fn from(value: PluginPaneCloseParams) -> Self {
            Self::PluginPaneClose(value)
        }
    }
    #[doc = "`ServerLiveHandoffParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct ServerLiveHandoffParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub expected_protocol: ::std::option::Option<u32>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub expected_version: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub import_exe: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`ServerSshAgentRegisterParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ServerSshAgentRegisterParams {
        #[doc = "Absolute remote-host agent socket. Registration lasts until this API connection closes."]
        pub socket_path: ::std::string::String,
    }
    #[doc = "`SplitDirection`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum SplitDirection {
        #[serde(rename = "right")]
        Right,
        #[serde(rename = "down")]
        Down,
    }
    impl ::std::fmt::Display for SplitDirection {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Right => f.write_str("right"),
                Self::Down => f.write_str("down"),
            }
        }
    }
    impl ::std::str::FromStr for SplitDirection {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "right" => Ok(Self::Right),
                "down" => Ok(Self::Down),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for SplitDirection {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for SplitDirection {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`Subscription`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "type")]
    pub enum Subscription {
        #[serde(rename = "workspace.created")]
        WorkspaceCreated,
        #[serde(rename = "workspace.updated")]
        WorkspaceUpdated,
        #[serde(rename = "workspace.metadata_updated")]
        WorkspaceMetadataUpdated,
        #[serde(rename = "workspace.renamed")]
        WorkspaceRenamed,
        #[serde(rename = "workspace.moved")]
        WorkspaceMoved,
        #[serde(rename = "workspace.reordered")]
        WorkspaceReordered,
        #[serde(rename = "workspace.closed")]
        WorkspaceClosed,
        #[serde(rename = "workspace.focused")]
        WorkspaceFocused,
        #[serde(rename = "worktree.created")]
        WorktreeCreated,
        #[serde(rename = "worktree.opened")]
        WorktreeOpened,
        #[serde(rename = "worktree.removed")]
        WorktreeRemoved,
        #[serde(rename = "tab.created")]
        TabCreated,
        #[serde(rename = "tab.closed")]
        TabClosed,
        #[serde(rename = "tab.focused")]
        TabFocused,
        #[serde(rename = "tab.renamed")]
        TabRenamed,
        #[serde(rename = "tab.moved")]
        TabMoved,
        #[serde(rename = "pane.created")]
        PaneCreated,
        #[serde(rename = "pane.closed")]
        PaneClosed,
        #[serde(rename = "pane.updated")]
        PaneUpdated,
        #[serde(rename = "pane.focused")]
        PaneFocused,
        #[serde(rename = "pane.moved")]
        PaneMoved,
        #[serde(rename = "pane.exited")]
        PaneExited,
        #[serde(rename = "pane.agent_detected")]
        PaneAgentDetected,
        #[serde(rename = "pane.output_matched")]
        PaneOutputMatched {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            lines: ::std::option::Option<u32>,
            #[serde(rename = "match")]
            match_: OutputMatch,
            pane_id: ::std::string::String,
            source: ReadSource,
            #[serde(default = "defaults::default_bool::<true>")]
            strip_ansi: bool,
        },
        #[serde(rename = "pane.agent_status_changed")]
        PaneAgentStatusChanged {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            agent_status: ::std::option::Option<AgentStatus>,
            pane_id: ::std::string::String,
        },
        #[serde(rename = "pane.scroll_changed")]
        PaneScrollChanged { pane_id: ::std::string::String },
        #[serde(rename = "layout.updated")]
        LayoutUpdated,
    }
    #[doc = "`TabCreateParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct TabCreateParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub env: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        #[serde(default)]
        pub focus: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub label: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`TabListParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct TabListParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`TabMoveParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct TabMoveParams {
        pub insert_index: u32,
        pub tab_id: ::std::string::String,
    }
    #[doc = "`TabRenameParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct TabRenameParams {
        pub label: ::std::string::String,
        pub tab_id: ::std::string::String,
    }
    #[doc = "`TabTarget`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct TabTarget {
        pub tab_id: ::std::string::String,
    }
    #[doc = "`ToastHerdrPosition`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum ToastHerdrPosition {
        #[serde(rename = "top-left")]
        TopLeft,
        #[serde(rename = "top-right")]
        TopRight,
        #[serde(rename = "bottom-left")]
        BottomLeft,
        #[serde(rename = "bottom-right")]
        BottomRight,
    }
    impl ::std::fmt::Display for ToastHerdrPosition {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::TopLeft => f.write_str("top-left"),
                Self::TopRight => f.write_str("top-right"),
                Self::BottomLeft => f.write_str("bottom-left"),
                Self::BottomRight => f.write_str("bottom-right"),
            }
        }
    }
    impl ::std::str::FromStr for ToastHerdrPosition {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "top-left" => Ok(Self::TopLeft),
                "top-right" => Ok(Self::TopRight),
                "bottom-left" => Ok(Self::BottomLeft),
                "bottom-right" => Ok(Self::BottomRight),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for ToastHerdrPosition {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for ToastHerdrPosition {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`WorkspaceCloseParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceCloseParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub close_group: ::std::option::Option<bool>,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`WorkspaceCreateParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct WorkspaceCreateParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub env: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        #[serde(default)]
        pub focus: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub label: ::std::option::Option<::std::string::String>,
        #[doc = "Workspace whose focused pane supplies the `follow` cwd policy."]
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub source_workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`WorkspaceMoveBlockParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceMoveBlockParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub before_workspace_id: ::std::option::Option<::std::string::String>,
        pub workspace_ids: ::std::vec::Vec<::std::string::String>,
    }
    #[doc = "`WorkspaceMoveParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceMoveParams {
        pub insert_index: u32,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`WorkspaceRenameParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceRenameParams {
        pub label: ::std::string::String,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`WorkspaceReportMetadataParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceReportMetadataParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub seq: ::std::option::Option<u64>,
        pub source: ::std::string::String,
        pub tokens: ::std::collections::HashMap<
            ::std::string::String,
            ::std::option::Option<::std::string::String>,
        >,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub ttl_ms: ::std::option::Option<::std::num::NonZeroU64>,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`WorkspaceTarget`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceTarget {
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`WorkspaceWorktreeInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceWorktreeInfo {
        pub checkout_path: ::std::string::String,
        pub is_linked_worktree: bool,
        pub repo_key: ::std::string::String,
        pub repo_name: ::std::string::String,
        pub repo_root: ::std::string::String,
    }
    #[doc = "`WorktreeCreateParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct WorktreeCreateParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub base: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub branch: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        #[serde(default)]
        pub focus: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub label: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub path: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub trust_repository: ::std::option::Option<bool>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`WorktreeListParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct WorktreeListParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub trust_repository: ::std::option::Option<bool>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`WorktreeOpenParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct WorktreeOpenParams {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub branch: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        #[serde(default)]
        pub focus: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub label: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub path: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub trust_repository: ::std::option::Option<bool>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`WorktreeRemoveParams`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorktreeRemoveParams {
        #[serde(default)]
        pub force: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub trust_repository: ::std::option::Option<bool>,
        pub workspace_id: ::std::string::String,
    }
    #[doc = " Generation of default values for serde."]
    pub mod defaults {
        pub(super) fn default_bool<const V: bool>() -> bool {
            V
        }
        pub(super) fn agent_read_params_format() -> super::ReadFormat {
            super::ReadFormat::Text
        }
        pub(super) fn agent_view_sort_order() -> super::AgentViewSortOrder {
            super::AgentViewSortOrder::Asc
        }
        pub(super) fn pane_read_params_format() -> super::ReadFormat {
            super::ReadFormat::Text
        }
        pub(super) fn pane_split_params_right_click() -> super::PaneRightClickTarget {
            super::PaneRightClickTarget::Herdr
        }
        pub(super) fn pane_zoom_params_mode() -> super::PaneZoomMode {
            super::PaneZoomMode::Toggle
        }
        pub(super) fn plugin_source_info_kind() -> super::PluginSourceKind {
            super::PluginSourceKind::Local
        }
    }
    #[doc = " Error types."]
    pub mod error {
        #[doc = r" Error from a `TryFrom` or `FromStr` implementation."]
        pub struct ConversionError(::std::borrow::Cow<'static, str>);
        impl ::std::error::Error for ConversionError {}
        impl ::std::fmt::Display for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Display::fmt(&self.0, f)
            }
        }
        impl ::std::fmt::Debug for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Debug::fmt(&self.0, f)
            }
        }
        impl From<&'static str> for ConversionError {
            fn from(value: &'static str) -> Self {
                Self(value.into())
            }
        }
        impl From<String> for ConversionError {
            fn from(value: String) -> Self {
                Self(value.into())
            }
        }
    }
}

/// Types generated from the `success_response` schema.
pub mod success_response {

    #[doc = "`AgentInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentInfo {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent_session: ::std::option::Option<AgentSessionInfo>,
        pub agent_status: AgentStatus,
        #[doc = "The current idle transition completed work, independently of who has viewed it."]
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub completion_seq: ::std::option::Option<u64>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub display_agent: ::std::option::Option<::std::string::String>,
        pub focused: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub foreground_cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub interactive_ready: ::std::option::Option<bool>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub launch_pending: ::std::option::Option<bool>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        pub pane_id: ::std::string::String,
        pub revision: u64,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub screen_detection_skipped: ::std::option::Option<bool>,
        #[serde(default)]
        pub state_change_seq: u64,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub state_labels: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        pub tab_id: ::std::string::String,
        pub terminal_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub terminal_title: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub terminal_title_stripped: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub title: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub tokens: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`AgentManifestInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentManifestInfo {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub active_version: ::std::option::Option<::std::string::String>,
        pub agent: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cached_remote_version: ::std::option::Option<::std::string::String>,
        pub local_override_shadowing_remote: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub remote_last_checked_unix: ::std::option::Option<u64>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub remote_update_error: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub remote_update_result: ::std::option::Option<::std::string::String>,
        pub source: ::std::string::String,
        pub source_kind: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub warning: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`AgentSessionInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentSessionInfo {
        pub agent: ::std::string::String,
        pub kind: AgentSessionRefKind,
        pub source: ::std::string::String,
        pub value: ::std::string::String,
    }
    #[doc = "`AgentSessionRefKind`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum AgentSessionRefKind {
        Variant0(AgentSessionRefKindVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for AgentSessionRefKind {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<AgentSessionRefKindVariant0> for AgentSessionRefKind {
        fn from(value: AgentSessionRefKindVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`AgentSessionRefKindVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum AgentSessionRefKindVariant0 {
        #[serde(rename = "id")]
        Id,
        #[serde(rename = "path")]
        Path,
    }
    impl ::std::fmt::Display for AgentSessionRefKindVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Id => f.write_str("id"),
                Self::Path => f.write_str("path"),
            }
        }
    }
    impl ::std::str::FromStr for AgentSessionRefKindVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "id" => Ok(Self::Id),
                "path" => Ok(Self::Path),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for AgentSessionRefKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for AgentSessionRefKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`AgentStatus`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum AgentStatus {
        Variant0(AgentStatusVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for AgentStatus {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<AgentStatusVariant0> for AgentStatus {
        fn from(value: AgentStatusVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`AgentStatusVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum AgentStatusVariant0 {
        #[serde(rename = "idle")]
        Idle,
        #[serde(rename = "working")]
        Working,
        #[serde(rename = "blocked")]
        Blocked,
        #[serde(rename = "done")]
        Done,
        #[serde(rename = "unknown")]
        Unknown,
    }
    impl ::std::fmt::Display for AgentStatusVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Idle => f.write_str("idle"),
                Self::Working => f.write_str("working"),
                Self::Blocked => f.write_str("blocked"),
                Self::Done => f.write_str("done"),
                Self::Unknown => f.write_str("unknown"),
            }
        }
    }
    impl ::std::str::FromStr for AgentStatusVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "idle" => Ok(Self::Idle),
                "working" => Ok(Self::Working),
                "blocked" => Ok(Self::Blocked),
                "done" => Ok(Self::Done),
                "unknown" => Ok(Self::Unknown),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for AgentStatusVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for AgentStatusVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`ClientWindowTitleReason`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum ClientWindowTitleReason {
        Variant0(ClientWindowTitleReasonVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for ClientWindowTitleReason {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<ClientWindowTitleReasonVariant0> for ClientWindowTitleReason {
        fn from(value: ClientWindowTitleReasonVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`ClientWindowTitleReasonVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum ClientWindowTitleReasonVariant0 {
        #[serde(rename = "set")]
        Set,
        #[serde(rename = "cleared")]
        Cleared,
        #[serde(rename = "no_foreground_client")]
        NoForegroundClient,
    }
    impl ::std::fmt::Display for ClientWindowTitleReasonVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Set => f.write_str("set"),
                Self::Cleared => f.write_str("cleared"),
                Self::NoForegroundClient => f.write_str("no_foreground_client"),
            }
        }
    }
    impl ::std::str::FromStr for ClientWindowTitleReasonVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "set" => Ok(Self::Set),
                "cleared" => Ok(Self::Cleared),
                "no_foreground_client" => Ok(Self::NoForegroundClient),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for ClientWindowTitleReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for ClientWindowTitleReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`ConfigReloadStatus`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum ConfigReloadStatus {
        Variant0(ConfigReloadStatusVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for ConfigReloadStatus {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<ConfigReloadStatusVariant0> for ConfigReloadStatus {
        fn from(value: ConfigReloadStatusVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`ConfigReloadStatusVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum ConfigReloadStatusVariant0 {
        #[serde(rename = "applied")]
        Applied,
        #[serde(rename = "partial")]
        Partial,
        #[serde(rename = "failed")]
        Failed,
    }
    impl ::std::fmt::Display for ConfigReloadStatusVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Applied => f.write_str("applied"),
                Self::Partial => f.write_str("partial"),
                Self::Failed => f.write_str("failed"),
            }
        }
    }
    impl ::std::str::FromStr for ConfigReloadStatusVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "applied" => Ok(Self::Applied),
                "partial" => Ok(Self::Partial),
                "failed" => Ok(Self::Failed),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for ConfigReloadStatusVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for ConfigReloadStatusVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`EventData`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "type")]
    pub enum EventData {
        #[serde(rename = "workspace_created")]
        WorkspaceCreated { workspace: WorkspaceInfo },
        #[serde(rename = "workspace_updated")]
        WorkspaceUpdated { workspace: WorkspaceInfo },
        #[serde(rename = "workspace_metadata_updated")]
        WorkspaceMetadataUpdated { workspace: WorkspaceInfo },
        #[serde(rename = "workspace_closed")]
        WorkspaceClosed {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            workspace: ::std::option::Option<WorkspaceInfo>,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "workspace_renamed")]
        WorkspaceRenamed {
            label: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "workspace_moved")]
        WorkspaceMoved {
            insert_index: u32,
            workspace_id: ::std::string::String,
            workspaces: ::std::vec::Vec<WorkspaceInfo>,
        },
        #[serde(rename = "workspace_reordered")]
        WorkspaceReordered {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            before_workspace_id: ::std::option::Option<::std::string::String>,
            workspace_ids: ::std::vec::Vec<::std::string::String>,
            workspaces: ::std::vec::Vec<WorkspaceInfo>,
        },
        #[serde(rename = "workspace_focused")]
        WorkspaceFocused { workspace_id: ::std::string::String },
        #[serde(rename = "worktree_created")]
        WorktreeCreated {
            workspace: WorkspaceInfo,
            worktree: WorktreeInfo,
        },
        #[serde(rename = "worktree_opened")]
        WorktreeOpened {
            already_open: bool,
            workspace: WorkspaceInfo,
            worktree: WorktreeInfo,
        },
        #[serde(rename = "worktree_removed")]
        WorktreeRemoved {
            forced: bool,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            workspace: ::std::option::Option<WorkspaceInfo>,
            workspace_id: ::std::string::String,
            worktree: WorktreeInfo,
        },
        #[serde(rename = "tab_created")]
        TabCreated { tab: TabInfo },
        #[serde(rename = "tab_closed")]
        TabClosed {
            tab_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "tab_renamed")]
        TabRenamed {
            label: ::std::string::String,
            tab_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "tab_moved")]
        TabMoved {
            insert_index: u32,
            tab_id: ::std::string::String,
            tabs: ::std::vec::Vec<TabInfo>,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "tab_focused")]
        TabFocused {
            tab_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_created")]
        PaneCreated { pane: PaneInfo },
        #[serde(rename = "pane_closed")]
        PaneClosed {
            pane_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_updated")]
        PaneUpdated { pane: PaneInfo },
        #[serde(rename = "pane_focused")]
        PaneFocused {
            pane_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_moved")]
        PaneMoved {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            closed_tab_id: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            closed_workspace_id: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            created_tab: ::std::option::Option<TabInfo>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            created_workspace: ::std::option::Option<WorkspaceInfo>,
            pane: PaneInfo,
            previous_pane_id: ::std::string::String,
            previous_tab_id: ::std::string::String,
            previous_workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_output_changed")]
        PaneOutputChanged {
            pane_id: ::std::string::String,
            revision: u64,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_exited")]
        PaneExited {
            pane_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_agent_detected")]
        PaneAgentDetected {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            agent: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            final_status: ::std::option::Option<AgentStatus>,
            pane_id: ::std::string::String,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            released: ::std::option::Option<bool>,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_agent_status_changed")]
        PaneAgentStatusChanged {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            agent: ::std::option::Option<::std::string::String>,
            agent_status: AgentStatus,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            display_agent: ::std::option::Option<::std::string::String>,
            pane_id: ::std::string::String,
            #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
            state_labels: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            title: ::std::option::Option<::std::string::String>,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "layout_updated")]
        LayoutUpdated { layout: PaneLayoutSnapshot },
    }
    #[doc = "`EventEnvelope`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct EventEnvelope {
        pub data: EventData,
        pub event: EventKind,
    }
    #[doc = "`EventKind`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum EventKind {
        Variant0(EventKindVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for EventKind {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<EventKindVariant0> for EventKind {
        fn from(value: EventKindVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`EventKindVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum EventKindVariant0 {
        #[serde(rename = "workspace_created")]
        WorkspaceCreated,
        #[serde(rename = "workspace_updated")]
        WorkspaceUpdated,
        #[serde(rename = "workspace_metadata_updated")]
        WorkspaceMetadataUpdated,
        #[serde(rename = "workspace_closed")]
        WorkspaceClosed,
        #[serde(rename = "workspace_renamed")]
        WorkspaceRenamed,
        #[serde(rename = "workspace_moved")]
        WorkspaceMoved,
        #[serde(rename = "workspace_reordered")]
        WorkspaceReordered,
        #[serde(rename = "workspace_focused")]
        WorkspaceFocused,
        #[serde(rename = "worktree_created")]
        WorktreeCreated,
        #[serde(rename = "worktree_opened")]
        WorktreeOpened,
        #[serde(rename = "worktree_removed")]
        WorktreeRemoved,
        #[serde(rename = "tab_created")]
        TabCreated,
        #[serde(rename = "tab_closed")]
        TabClosed,
        #[serde(rename = "tab_renamed")]
        TabRenamed,
        #[serde(rename = "tab_moved")]
        TabMoved,
        #[serde(rename = "tab_focused")]
        TabFocused,
        #[serde(rename = "pane_created")]
        PaneCreated,
        #[serde(rename = "pane_closed")]
        PaneClosed,
        #[serde(rename = "pane_updated")]
        PaneUpdated,
        #[serde(rename = "pane_focused")]
        PaneFocused,
        #[serde(rename = "pane_moved")]
        PaneMoved,
        #[serde(rename = "pane_output_changed")]
        PaneOutputChanged,
        #[serde(rename = "pane_exited")]
        PaneExited,
        #[serde(rename = "pane_agent_detected")]
        PaneAgentDetected,
        #[serde(rename = "pane_agent_status_changed")]
        PaneAgentStatusChanged,
        #[serde(rename = "layout_updated")]
        LayoutUpdated,
    }
    impl ::std::fmt::Display for EventKindVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::WorkspaceCreated => f.write_str("workspace_created"),
                Self::WorkspaceUpdated => f.write_str("workspace_updated"),
                Self::WorkspaceMetadataUpdated => f.write_str("workspace_metadata_updated"),
                Self::WorkspaceClosed => f.write_str("workspace_closed"),
                Self::WorkspaceRenamed => f.write_str("workspace_renamed"),
                Self::WorkspaceMoved => f.write_str("workspace_moved"),
                Self::WorkspaceReordered => f.write_str("workspace_reordered"),
                Self::WorkspaceFocused => f.write_str("workspace_focused"),
                Self::WorktreeCreated => f.write_str("worktree_created"),
                Self::WorktreeOpened => f.write_str("worktree_opened"),
                Self::WorktreeRemoved => f.write_str("worktree_removed"),
                Self::TabCreated => f.write_str("tab_created"),
                Self::TabClosed => f.write_str("tab_closed"),
                Self::TabRenamed => f.write_str("tab_renamed"),
                Self::TabMoved => f.write_str("tab_moved"),
                Self::TabFocused => f.write_str("tab_focused"),
                Self::PaneCreated => f.write_str("pane_created"),
                Self::PaneClosed => f.write_str("pane_closed"),
                Self::PaneUpdated => f.write_str("pane_updated"),
                Self::PaneFocused => f.write_str("pane_focused"),
                Self::PaneMoved => f.write_str("pane_moved"),
                Self::PaneOutputChanged => f.write_str("pane_output_changed"),
                Self::PaneExited => f.write_str("pane_exited"),
                Self::PaneAgentDetected => f.write_str("pane_agent_detected"),
                Self::PaneAgentStatusChanged => f.write_str("pane_agent_status_changed"),
                Self::LayoutUpdated => f.write_str("layout_updated"),
            }
        }
    }
    impl ::std::str::FromStr for EventKindVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "workspace_created" => Ok(Self::WorkspaceCreated),
                "workspace_updated" => Ok(Self::WorkspaceUpdated),
                "workspace_metadata_updated" => Ok(Self::WorkspaceMetadataUpdated),
                "workspace_closed" => Ok(Self::WorkspaceClosed),
                "workspace_renamed" => Ok(Self::WorkspaceRenamed),
                "workspace_moved" => Ok(Self::WorkspaceMoved),
                "workspace_reordered" => Ok(Self::WorkspaceReordered),
                "workspace_focused" => Ok(Self::WorkspaceFocused),
                "worktree_created" => Ok(Self::WorktreeCreated),
                "worktree_opened" => Ok(Self::WorktreeOpened),
                "worktree_removed" => Ok(Self::WorktreeRemoved),
                "tab_created" => Ok(Self::TabCreated),
                "tab_closed" => Ok(Self::TabClosed),
                "tab_renamed" => Ok(Self::TabRenamed),
                "tab_moved" => Ok(Self::TabMoved),
                "tab_focused" => Ok(Self::TabFocused),
                "pane_created" => Ok(Self::PaneCreated),
                "pane_closed" => Ok(Self::PaneClosed),
                "pane_updated" => Ok(Self::PaneUpdated),
                "pane_focused" => Ok(Self::PaneFocused),
                "pane_moved" => Ok(Self::PaneMoved),
                "pane_output_changed" => Ok(Self::PaneOutputChanged),
                "pane_exited" => Ok(Self::PaneExited),
                "pane_agent_detected" => Ok(Self::PaneAgentDetected),
                "pane_agent_status_changed" => Ok(Self::PaneAgentStatusChanged),
                "layout_updated" => Ok(Self::LayoutUpdated),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for EventKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for EventKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`InstalledPluginInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct InstalledPluginInfo {
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub actions: ::std::vec::Vec<PluginManifestAction>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub build: ::std::vec::Vec<PluginManifestBuild>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub description: ::std::option::Option<::std::string::String>,
        pub enabled: bool,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub events: ::std::vec::Vec<PluginManifestEventHook>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub link_handlers: ::std::vec::Vec<PluginManifestLinkHandler>,
        pub manifest_path: ::std::string::String,
        #[serde(default)]
        pub min_herdr_version: ::std::string::String,
        pub name: ::std::string::String,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub panes: ::std::vec::Vec<PluginManifestPane>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub platforms: ::std::option::Option<::std::vec::Vec<PluginPlatform>>,
        pub plugin_id: ::std::string::String,
        pub plugin_root: ::std::string::String,
        #[serde(default = "defaults::installed_plugin_info_source")]
        pub source: PluginSourceInfo,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub startup: ::std::vec::Vec<PluginManifestStartup>,
        pub version: ::std::string::String,
        #[doc = "Warnings collected at link time or on registry load (e.g. unknown event names,\nmissing manifest file). Non-fatal — the entry is kept and surfaced by plugin.list."]
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub warnings: ::std::vec::Vec<::std::string::String>,
    }
    #[doc = "`IntegrationInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct IntegrationInfo {
        pub available: bool,
        pub command: ::std::string::String,
        pub label: ::std::string::String,
        pub state: IntegrationState,
        pub target: IntegrationTarget,
    }
    #[doc = "`IntegrationInstallResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct IntegrationInstallResult {
        pub messages: ::std::vec::Vec<::std::string::String>,
    }
    #[doc = "`IntegrationState`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum IntegrationState {
        Variant0(IntegrationStateVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for IntegrationState {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<IntegrationStateVariant0> for IntegrationState {
        fn from(value: IntegrationStateVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`IntegrationStateVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum IntegrationStateVariant0 {
        #[serde(rename = "not_installed")]
        NotInstalled,
        #[serde(rename = "current")]
        Current,
        #[serde(rename = "outdated")]
        Outdated,
    }
    impl ::std::fmt::Display for IntegrationStateVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::NotInstalled => f.write_str("not_installed"),
                Self::Current => f.write_str("current"),
                Self::Outdated => f.write_str("outdated"),
            }
        }
    }
    impl ::std::str::FromStr for IntegrationStateVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "not_installed" => Ok(Self::NotInstalled),
                "current" => Ok(Self::Current),
                "outdated" => Ok(Self::Outdated),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for IntegrationStateVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for IntegrationStateVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`IntegrationTarget`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum IntegrationTarget {
        Variant0(IntegrationTargetVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for IntegrationTarget {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<IntegrationTargetVariant0> for IntegrationTarget {
        fn from(value: IntegrationTargetVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`IntegrationTargetVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum IntegrationTargetVariant0 {
        #[serde(rename = "pi")]
        Pi,
        #[serde(rename = "omp")]
        Omp,
        #[serde(rename = "claude")]
        Claude,
        #[serde(rename = "codex")]
        Codex,
        #[serde(rename = "copilot")]
        Copilot,
        #[serde(rename = "devin")]
        Devin,
        #[serde(rename = "droid")]
        Droid,
        #[serde(rename = "kimi")]
        Kimi,
        #[serde(rename = "opencode")]
        Opencode,
        #[serde(rename = "kilo")]
        Kilo,
        #[serde(rename = "hermes")]
        Hermes,
        #[serde(rename = "qodercli")]
        Qodercli,
        #[serde(rename = "qwen")]
        Qwen,
        #[serde(rename = "cursor")]
        Cursor,
        #[serde(rename = "mastracode")]
        Mastracode,
        #[serde(rename = "antigravity_cli")]
        AntigravityCli,
        #[serde(rename = "grok")]
        Grok,
    }
    impl ::std::fmt::Display for IntegrationTargetVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Pi => f.write_str("pi"),
                Self::Omp => f.write_str("omp"),
                Self::Claude => f.write_str("claude"),
                Self::Codex => f.write_str("codex"),
                Self::Copilot => f.write_str("copilot"),
                Self::Devin => f.write_str("devin"),
                Self::Droid => f.write_str("droid"),
                Self::Kimi => f.write_str("kimi"),
                Self::Opencode => f.write_str("opencode"),
                Self::Kilo => f.write_str("kilo"),
                Self::Hermes => f.write_str("hermes"),
                Self::Qodercli => f.write_str("qodercli"),
                Self::Qwen => f.write_str("qwen"),
                Self::Cursor => f.write_str("cursor"),
                Self::Mastracode => f.write_str("mastracode"),
                Self::AntigravityCli => f.write_str("antigravity_cli"),
                Self::Grok => f.write_str("grok"),
            }
        }
    }
    impl ::std::str::FromStr for IntegrationTargetVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "pi" => Ok(Self::Pi),
                "omp" => Ok(Self::Omp),
                "claude" => Ok(Self::Claude),
                "codex" => Ok(Self::Codex),
                "copilot" => Ok(Self::Copilot),
                "devin" => Ok(Self::Devin),
                "droid" => Ok(Self::Droid),
                "kimi" => Ok(Self::Kimi),
                "opencode" => Ok(Self::Opencode),
                "kilo" => Ok(Self::Kilo),
                "hermes" => Ok(Self::Hermes),
                "qodercli" => Ok(Self::Qodercli),
                "qwen" => Ok(Self::Qwen),
                "cursor" => Ok(Self::Cursor),
                "mastracode" => Ok(Self::Mastracode),
                "antigravity_cli" => Ok(Self::AntigravityCli),
                "grok" => Ok(Self::Grok),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for IntegrationTargetVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for IntegrationTargetVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`IntegrationUninstallResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct IntegrationUninstallResult {
        pub messages: ::std::vec::Vec<::std::string::String>,
    }
    #[doc = "`LayoutDescription`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct LayoutDescription {
        pub focused_pane_id: ::std::string::String,
        pub root: LayoutNode,
        pub tab_id: ::std::string::String,
        pub workspace_id: ::std::string::String,
        pub zoomed: bool,
    }
    #[doc = "`LayoutNode`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "type")]
    pub enum LayoutNode {
        #[serde(rename = "pane")]
        Pane {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            command: ::std::option::Option<::std::vec::Vec<::std::string::String>>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            cwd: ::std::option::Option<::std::string::String>,
            #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
            env: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            label: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            pane_id: ::std::option::Option<::std::string::String>,
        },
        #[serde(rename = "split")]
        Split {
            direction: SplitDirection,
            first: ::std::boxed::Box<LayoutNode>,
            ratio: f32,
            second: ::std::boxed::Box<LayoutNode>,
        },
    }
    #[doc = "`NotificationShowReason`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum NotificationShowReason {
        Variant0(NotificationShowReasonVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for NotificationShowReason {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<NotificationShowReasonVariant0> for NotificationShowReason {
        fn from(value: NotificationShowReasonVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`NotificationShowReasonVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum NotificationShowReasonVariant0 {
        #[serde(rename = "shown")]
        Shown,
        #[serde(rename = "disabled")]
        Disabled,
        #[serde(rename = "rate_limited")]
        RateLimited,
        #[serde(rename = "no_foreground_client")]
        NoForegroundClient,
        #[serde(rename = "busy")]
        Busy,
    }
    impl ::std::fmt::Display for NotificationShowReasonVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Shown => f.write_str("shown"),
                Self::Disabled => f.write_str("disabled"),
                Self::RateLimited => f.write_str("rate_limited"),
                Self::NoForegroundClient => f.write_str("no_foreground_client"),
                Self::Busy => f.write_str("busy"),
            }
        }
    }
    impl ::std::str::FromStr for NotificationShowReasonVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "shown" => Ok(Self::Shown),
                "disabled" => Ok(Self::Disabled),
                "rate_limited" => Ok(Self::RateLimited),
                "no_foreground_client" => Ok(Self::NoForegroundClient),
                "busy" => Ok(Self::Busy),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for NotificationShowReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for NotificationShowReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneDirection`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PaneDirection {
        Variant0(PaneDirectionVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PaneDirection {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PaneDirectionVariant0> for PaneDirection {
        fn from(value: PaneDirectionVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PaneDirectionVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneDirectionVariant0 {
        #[serde(rename = "left")]
        Left,
        #[serde(rename = "right")]
        Right,
        #[serde(rename = "up")]
        Up,
        #[serde(rename = "down")]
        Down,
    }
    impl ::std::fmt::Display for PaneDirectionVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Left => f.write_str("left"),
                Self::Right => f.write_str("right"),
                Self::Up => f.write_str("up"),
                Self::Down => f.write_str("down"),
            }
        }
    }
    impl ::std::str::FromStr for PaneDirectionVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "left" => Ok(Self::Left),
                "right" => Ok(Self::Right),
                "up" => Ok(Self::Up),
                "down" => Ok(Self::Down),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneDirectionVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneDirectionVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneEdgesResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneEdgesResult {
        pub down: bool,
        pub layout: PaneLayoutSnapshot,
        pub left: bool,
        pub pane_id: ::std::string::String,
        pub right: bool,
        pub up: bool,
    }
    #[doc = "`PaneFocusDirectionReason`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PaneFocusDirectionReason {
        Variant0(PaneFocusDirectionReasonVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PaneFocusDirectionReason {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PaneFocusDirectionReasonVariant0> for PaneFocusDirectionReason {
        fn from(value: PaneFocusDirectionReasonVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PaneFocusDirectionReasonVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneFocusDirectionReasonVariant0 {
        #[serde(rename = "no_neighbor")]
        NoNeighbor,
    }
    impl ::std::fmt::Display for PaneFocusDirectionReasonVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::NoNeighbor => f.write_str("no_neighbor"),
            }
        }
    }
    impl ::std::str::FromStr for PaneFocusDirectionReasonVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "no_neighbor" => Ok(Self::NoNeighbor),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneFocusDirectionReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneFocusDirectionReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneFocusDirectionResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneFocusDirectionResult {
        pub changed: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_pane_id: ::std::option::Option<::std::string::String>,
        pub layout: PaneLayoutSnapshot,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub reason: ::std::option::Option<PaneFocusDirectionReason>,
        pub source_pane_id: ::std::string::String,
    }
    #[doc = "`PaneInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneInfo {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent_session: ::std::option::Option<AgentSessionInfo>,
        pub agent_status: AgentStatus,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub display_agent: ::std::option::Option<::std::string::String>,
        pub focused: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub foreground_cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub label: ::std::option::Option<::std::string::String>,
        pub pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub restore_error: ::std::option::Option<::std::string::String>,
        pub revision: u64,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub scroll: ::std::option::Option<PaneScrollInfo>,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub state_labels: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        pub tab_id: ::std::string::String,
        pub terminal_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub terminal_title: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub terminal_title_stripped: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub title: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub tokens: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`PaneLayoutPane`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneLayoutPane {
        pub focused: bool,
        pub pane_id: ::std::string::String,
        pub rect: PaneLayoutRect,
    }
    #[doc = "`PaneLayoutRect`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneLayoutRect {
        pub height: u16,
        pub width: u16,
        pub x: u16,
        pub y: u16,
    }
    #[doc = "`PaneLayoutSnapshot`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneLayoutSnapshot {
        pub area: PaneLayoutRect,
        pub focused_pane_id: ::std::string::String,
        pub panes: ::std::vec::Vec<PaneLayoutPane>,
        pub splits: ::std::vec::Vec<PaneLayoutSplit>,
        pub tab_id: ::std::string::String,
        pub workspace_id: ::std::string::String,
        pub zoomed: bool,
    }
    #[doc = "`PaneLayoutSplit`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneLayoutSplit {
        pub direction: SplitDirection,
        pub id: ::std::string::String,
        pub ratio: f32,
        pub rect: PaneLayoutRect,
    }
    #[doc = "Inclusive display-cell columns on a pane's current viewport."]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneLinkRegion {
        pub end_col: u16,
        pub row: u16,
        pub start_col: u16,
    }
    #[doc = "`PaneMoveReason`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PaneMoveReason {
        Variant0(PaneMoveReasonVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PaneMoveReason {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PaneMoveReasonVariant0> for PaneMoveReason {
        fn from(value: PaneMoveReasonVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PaneMoveReasonVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneMoveReasonVariant0 {
        #[serde(rename = "same_tab")]
        SameTab,
        #[serde(rename = "zoomed_tab")]
        ZoomedTab,
    }
    impl ::std::fmt::Display for PaneMoveReasonVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::SameTab => f.write_str("same_tab"),
                Self::ZoomedTab => f.write_str("zoomed_tab"),
            }
        }
    }
    impl ::std::str::FromStr for PaneMoveReasonVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "same_tab" => Ok(Self::SameTab),
                "zoomed_tab" => Ok(Self::ZoomedTab),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneMoveReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneMoveReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneMoveResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneMoveResult {
        pub changed: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub closed_tab_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub closed_workspace_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub created_tab: ::std::option::Option<TabInfo>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub created_workspace: ::std::option::Option<WorkspaceInfo>,
        pub focused_pane_id: ::std::string::String,
        pub pane: PaneInfo,
        pub previous_pane_id: ::std::string::String,
        pub previous_tab_id: ::std::string::String,
        pub previous_workspace_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub reason: ::std::option::Option<PaneMoveReason>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub source_layout: ::std::option::Option<PaneLayoutSnapshot>,
        pub target_layout: PaneLayoutSnapshot,
    }
    #[doc = "`PaneNeighborResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneNeighborResult {
        pub direction: PaneDirection,
        pub layout: PaneLayoutSnapshot,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub neighbor_pane_id: ::std::option::Option<::std::string::String>,
        pub pane_id: ::std::string::String,
    }
    #[doc = "`PaneProcessInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneProcessInfo {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub foreground_process_group_id: ::std::option::Option<u32>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub foreground_processes: ::std::vec::Vec<PaneProcessInfoProcess>,
        pub pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub shell_pid: ::std::option::Option<u32>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub tty: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneProcessInfoProcess`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneProcessInfoProcess {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub argv: ::std::option::Option<::std::vec::Vec<::std::string::String>>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub argv0: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cmdline: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        pub name: ::std::string::String,
        pub pid: u32,
    }
    #[doc = "`PaneReadResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneReadResult {
        pub format: ReadFormat,
        pub pane_id: ::std::string::String,
        pub revision: u64,
        pub source: ReadSource,
        pub tab_id: ::std::string::String,
        pub text: ::std::string::String,
        pub truncated: bool,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`PaneResizeReason`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PaneResizeReason {
        Variant0(PaneResizeReasonVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PaneResizeReason {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PaneResizeReasonVariant0> for PaneResizeReason {
        fn from(value: PaneResizeReasonVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PaneResizeReasonVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneResizeReasonVariant0 {
        #[serde(rename = "unchanged")]
        Unchanged,
    }
    impl ::std::fmt::Display for PaneResizeReasonVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Unchanged => f.write_str("unchanged"),
            }
        }
    }
    impl ::std::str::FromStr for PaneResizeReasonVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "unchanged" => Ok(Self::Unchanged),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneResizeReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneResizeReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneResizeResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneResizeResult {
        pub changed: bool,
        pub focused_pane_id: ::std::string::String,
        pub layout: PaneLayoutSnapshot,
        pub pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub reason: ::std::option::Option<PaneResizeReason>,
    }
    #[doc = "`PaneScrollInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneScrollInfo {
        pub max_offset_from_bottom: u64,
        pub offset_from_bottom: u64,
        pub viewport_rows: u64,
    }
    #[doc = "`PaneSwapReason`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PaneSwapReason {
        Variant0(PaneSwapReasonVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PaneSwapReason {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PaneSwapReasonVariant0> for PaneSwapReason {
        fn from(value: PaneSwapReasonVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PaneSwapReasonVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneSwapReasonVariant0 {
        #[serde(rename = "no_neighbor")]
        NoNeighbor,
        #[serde(rename = "same_pane")]
        SamePane,
        #[serde(rename = "not_found")]
        NotFound,
        #[serde(rename = "cross_tab")]
        CrossTab,
    }
    impl ::std::fmt::Display for PaneSwapReasonVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::NoNeighbor => f.write_str("no_neighbor"),
                Self::SamePane => f.write_str("same_pane"),
                Self::NotFound => f.write_str("not_found"),
                Self::CrossTab => f.write_str("cross_tab"),
            }
        }
    }
    impl ::std::str::FromStr for PaneSwapReasonVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "no_neighbor" => Ok(Self::NoNeighbor),
                "same_pane" => Ok(Self::SamePane),
                "not_found" => Ok(Self::NotFound),
                "cross_tab" => Ok(Self::CrossTab),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneSwapReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneSwapReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneSwapResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneSwapResult {
        pub changed: bool,
        pub focused_pane_id: ::std::string::String,
        pub layout: PaneLayoutSnapshot,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub reason: ::std::option::Option<PaneSwapReason>,
        pub source_pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub target_pane_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PaneTextPoint`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneTextPoint {
        pub col: u16,
        pub row: u32,
    }
    #[doc = "`PaneTextRange`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneTextRange {
        pub end: PaneTextPoint,
        pub start: PaneTextPoint,
    }
    #[doc = "`PaneZoomReason`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PaneZoomReason {
        Variant0(PaneZoomReasonVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PaneZoomReason {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PaneZoomReasonVariant0> for PaneZoomReason {
        fn from(value: PaneZoomReasonVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PaneZoomReasonVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PaneZoomReasonVariant0 {
        #[serde(rename = "single_pane")]
        SinglePane,
        #[serde(rename = "already_zoomed")]
        AlreadyZoomed,
        #[serde(rename = "already_unzoomed")]
        AlreadyUnzoomed,
    }
    impl ::std::fmt::Display for PaneZoomReasonVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::SinglePane => f.write_str("single_pane"),
                Self::AlreadyZoomed => f.write_str("already_zoomed"),
                Self::AlreadyUnzoomed => f.write_str("already_unzoomed"),
            }
        }
    }
    impl ::std::str::FromStr for PaneZoomReasonVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "single_pane" => Ok(Self::SinglePane),
                "already_zoomed" => Ok(Self::AlreadyZoomed),
                "already_unzoomed" => Ok(Self::AlreadyUnzoomed),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PaneZoomReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PaneZoomReasonVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneZoomResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneZoomResult {
        pub changed: bool,
        pub focus_changed: bool,
        pub focused_pane_id: ::std::string::String,
        pub layout: PaneLayoutSnapshot,
        pub pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub reason: ::std::option::Option<PaneZoomReason>,
        pub zoom_changed: bool,
        pub zoomed: bool,
    }
    #[doc = "`PluginActionContext`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PluginActionContext {
        Variant0(PluginActionContextVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PluginActionContext {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PluginActionContextVariant0> for PluginActionContext {
        fn from(value: PluginActionContextVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PluginActionContextVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PluginActionContextVariant0 {
        #[serde(rename = "global")]
        Global,
        #[serde(rename = "workspace")]
        Workspace,
        #[serde(rename = "tab")]
        Tab,
        #[serde(rename = "pane")]
        Pane,
        #[serde(rename = "selection")]
        Selection,
    }
    impl ::std::fmt::Display for PluginActionContextVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Global => f.write_str("global"),
                Self::Workspace => f.write_str("workspace"),
                Self::Tab => f.write_str("tab"),
                Self::Pane => f.write_str("pane"),
                Self::Selection => f.write_str("selection"),
            }
        }
    }
    impl ::std::str::FromStr for PluginActionContextVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "global" => Ok(Self::Global),
                "workspace" => Ok(Self::Workspace),
                "tab" => Ok(Self::Tab),
                "pane" => Ok(Self::Pane),
                "selection" => Ok(Self::Selection),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PluginActionContextVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PluginActionContextVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PluginActionInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginActionInfo {
        pub action_id: ::std::string::String,
        pub command: ::std::vec::Vec<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub contexts: ::std::vec::Vec<PluginActionContext>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub description: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub platforms: ::std::option::Option<::std::vec::Vec<PluginPlatform>>,
        pub plugin_id: ::std::string::String,
        pub title: ::std::string::String,
    }
    #[doc = "`PluginCommandLogInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginCommandLogInfo {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub action_id: ::std::option::Option<::std::string::String>,
        pub command: ::std::vec::Vec<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub error: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub event: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub exit_code: ::std::option::Option<i32>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub finished_unix_ms: ::std::option::Option<u64>,
        pub log_id: ::std::string::String,
        pub plugin_id: ::std::string::String,
        pub started_unix_ms: u64,
        pub status: PluginCommandStatus,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub stderr: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub stdout: ::std::option::Option<::std::string::String>,
    }
    #[doc = "`PluginCommandStatus`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PluginCommandStatus {
        Variant0(PluginCommandStatusVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PluginCommandStatus {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PluginCommandStatusVariant0> for PluginCommandStatus {
        fn from(value: PluginCommandStatusVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PluginCommandStatusVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PluginCommandStatusVariant0 {
        #[serde(rename = "running")]
        Running,
        #[serde(rename = "succeeded")]
        Succeeded,
        #[serde(rename = "failed")]
        Failed,
    }
    impl ::std::fmt::Display for PluginCommandStatusVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Running => f.write_str("running"),
                Self::Succeeded => f.write_str("succeeded"),
                Self::Failed => f.write_str("failed"),
            }
        }
    }
    impl ::std::str::FromStr for PluginCommandStatusVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "running" => Ok(Self::Running),
                "succeeded" => Ok(Self::Succeeded),
                "failed" => Ok(Self::Failed),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PluginCommandStatusVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PluginCommandStatusVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PluginInvocationContext`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
    pub struct PluginInvocationContext {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub clicked_url: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub correlation_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_pane_agent: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_pane_cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_pane_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_pane_status: ::std::option::Option<AgentStatus>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub invocation_source: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub link_handler_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub selected_text: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub tab_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub tab_label: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub workspace_label: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub worktree: ::std::option::Option<WorkspaceWorktreeInfo>,
    }
    #[doc = "`PluginManifestAction`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginManifestAction {
        pub command: ::std::vec::Vec<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub contexts: ::std::vec::Vec<PluginActionContext>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub description: ::std::option::Option<::std::string::String>,
        pub id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub platforms: ::std::option::Option<::std::vec::Vec<PluginPlatform>>,
        pub title: ::std::string::String,
    }
    #[doc = "`PluginManifestBuild`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginManifestBuild {
        pub command: ::std::vec::Vec<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub platforms: ::std::option::Option<::std::vec::Vec<PluginPlatform>>,
    }
    #[doc = "`PluginManifestEventHook`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginManifestEventHook {
        pub command: ::std::vec::Vec<::std::string::String>,
        pub on: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub platforms: ::std::option::Option<::std::vec::Vec<PluginPlatform>>,
    }
    #[doc = "`PluginManifestLinkHandler`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginManifestLinkHandler {
        pub action: ::std::string::String,
        pub id: ::std::string::String,
        pub pattern: ::serde_json::Value,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub platforms: ::std::option::Option<::std::vec::Vec<PluginPlatform>>,
        pub title: ::std::string::String,
    }
    #[doc = "`PluginManifestPane`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginManifestPane {
        pub command: ::std::vec::Vec<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub description: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub height: ::std::option::Option<PopupSize>,
        pub id: ::std::string::String,
        #[serde(default = "defaults::plugin_manifest_pane_placement")]
        pub placement: PluginPanePlacement,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub platforms: ::std::option::Option<::std::vec::Vec<PluginPlatform>>,
        pub title: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub width: ::std::option::Option<PopupSize>,
    }
    #[doc = "`PluginManifestStartup`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginManifestStartup {
        pub command: ::std::vec::Vec<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub platforms: ::std::option::Option<::std::vec::Vec<PluginPlatform>>,
    }
    #[doc = "`PluginPaneInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginPaneInfo {
        pub entrypoint: ::std::string::String,
        pub pane: PaneInfo,
        pub plugin_id: ::std::string::String,
    }
    #[doc = "`PluginPanePlacement`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PluginPanePlacement {
        Variant0(PluginPanePlacementVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PluginPanePlacement {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PluginPanePlacementVariant0> for PluginPanePlacement {
        fn from(value: PluginPanePlacementVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PluginPanePlacementVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PluginPanePlacementVariant0 {
        #[serde(rename = "overlay")]
        Overlay,
        #[serde(rename = "popup")]
        Popup,
        #[serde(rename = "split")]
        Split,
        #[serde(rename = "tab")]
        Tab,
        #[serde(rename = "zoomed")]
        Zoomed,
    }
    impl ::std::fmt::Display for PluginPanePlacementVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Overlay => f.write_str("overlay"),
                Self::Popup => f.write_str("popup"),
                Self::Split => f.write_str("split"),
                Self::Tab => f.write_str("tab"),
                Self::Zoomed => f.write_str("zoomed"),
            }
        }
    }
    impl ::std::str::FromStr for PluginPanePlacementVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "overlay" => Ok(Self::Overlay),
                "popup" => Ok(Self::Popup),
                "split" => Ok(Self::Split),
                "tab" => Ok(Self::Tab),
                "zoomed" => Ok(Self::Zoomed),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PluginPanePlacementVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PluginPanePlacementVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PluginPlatform`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PluginPlatform {
        Variant0(PluginPlatformVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PluginPlatform {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PluginPlatformVariant0> for PluginPlatform {
        fn from(value: PluginPlatformVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PluginPlatformVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PluginPlatformVariant0 {
        #[serde(rename = "linux")]
        Linux,
        #[serde(rename = "macos")]
        Macos,
        #[serde(rename = "windows")]
        Windows,
    }
    impl ::std::fmt::Display for PluginPlatformVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Linux => f.write_str("linux"),
                Self::Macos => f.write_str("macos"),
                Self::Windows => f.write_str("windows"),
            }
        }
    }
    impl ::std::str::FromStr for PluginPlatformVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "linux" => Ok(Self::Linux),
                "macos" => Ok(Self::Macos),
                "windows" => Ok(Self::Windows),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PluginPlatformVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PluginPlatformVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PluginSourceInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PluginSourceInfo {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub installed_unix_ms: ::std::option::Option<u64>,
        #[serde(default = "defaults::plugin_source_info_kind")]
        pub kind: PluginSourceKind,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub managed_path: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub owner: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub repo: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub requested_ref: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub resolved_commit: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub subdir: ::std::option::Option<::std::string::String>,
    }
    impl ::std::default::Default for PluginSourceInfo {
        fn default() -> Self {
            Self {
                installed_unix_ms: Default::default(),
                kind: defaults::plugin_source_info_kind(),
                managed_path: Default::default(),
                owner: Default::default(),
                repo: Default::default(),
                requested_ref: Default::default(),
                resolved_commit: Default::default(),
                subdir: Default::default(),
            }
        }
    }
    #[doc = "`PluginSourceKind`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PluginSourceKind {
        Variant0(PluginSourceKindVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for PluginSourceKind {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<PluginSourceKindVariant0> for PluginSourceKind {
        fn from(value: PluginSourceKindVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`PluginSourceKindVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum PluginSourceKindVariant0 {
        #[serde(rename = "local")]
        Local,
        #[serde(rename = "github")]
        Github,
    }
    impl ::std::fmt::Display for PluginSourceKindVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Local => f.write_str("local"),
                Self::Github => f.write_str("github"),
            }
        }
    }
    impl ::std::str::FromStr for PluginSourceKindVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "local" => Ok(Self::Local),
                "github" => Ok(Self::Github),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for PluginSourceKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for PluginSourceKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PopupSize`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum PopupSize {
        Integer(u16),
        String(::std::string::String),
    }
    impl ::std::fmt::Display for PopupSize {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Integer(x) => x.fmt(f),
                Self::String(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<u16> for PopupSize {
        fn from(value: u16) -> Self {
            Self::Integer(value)
        }
    }
    #[doc = "`ReadFormat`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum ReadFormat {
        Variant0(ReadFormatVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for ReadFormat {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<ReadFormatVariant0> for ReadFormat {
        fn from(value: ReadFormatVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`ReadFormatVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum ReadFormatVariant0 {
        #[serde(rename = "text")]
        Text,
        #[serde(rename = "ansi")]
        Ansi,
    }
    impl ::std::fmt::Display for ReadFormatVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Text => f.write_str("text"),
                Self::Ansi => f.write_str("ansi"),
            }
        }
    }
    impl ::std::str::FromStr for ReadFormatVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "text" => Ok(Self::Text),
                "ansi" => Ok(Self::Ansi),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for ReadFormatVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for ReadFormatVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`ReadSource`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum ReadSource {
        Variant0(ReadSourceVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for ReadSource {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<ReadSourceVariant0> for ReadSource {
        fn from(value: ReadSourceVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`ReadSourceVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum ReadSourceVariant0 {
        #[serde(rename = "visible")]
        Visible,
        #[serde(rename = "recent")]
        Recent,
        #[serde(rename = "recent_unwrapped")]
        RecentUnwrapped,
        #[serde(rename = "detection")]
        Detection,
    }
    impl ::std::fmt::Display for ReadSourceVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Visible => f.write_str("visible"),
                Self::Recent => f.write_str("recent"),
                Self::RecentUnwrapped => f.write_str("recent_unwrapped"),
                Self::Detection => f.write_str("detection"),
            }
        }
    }
    impl ::std::str::FromStr for ReadSourceVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "visible" => Ok(Self::Visible),
                "recent" => Ok(Self::Recent),
                "recent_unwrapped" => Ok(Self::RecentUnwrapped),
                "detection" => Ok(Self::Detection),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for ReadSourceVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for ReadSourceVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`ResponseResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "type")]
    pub enum ResponseResult {
        #[serde(rename = "pong")]
        Pong {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            capabilities: ::std::option::Option<ServerCapabilities>,
            protocol: u32,
            version: ::std::string::String,
        },
        #[serde(rename = "session_snapshot")]
        SessionSnapshot { snapshot: SessionSnapshot },
        #[serde(rename = "workspace_info")]
        WorkspaceInfo { workspace: WorkspaceInfo },
        #[serde(rename = "workspace_created")]
        WorkspaceCreated {
            root_pane: PaneInfo,
            tab: TabInfo,
            workspace: WorkspaceInfo,
        },
        #[serde(rename = "workspace_list")]
        WorkspaceList {
            workspaces: ::std::vec::Vec<WorkspaceInfo>,
        },
        #[serde(rename = "worktree_list")]
        WorktreeList {
            source: WorktreeSourceInfo,
            worktrees: ::std::vec::Vec<WorktreeInfo>,
        },
        #[serde(rename = "worktree_created")]
        WorktreeCreated {
            root_pane: PaneInfo,
            tab: TabInfo,
            workspace: WorkspaceInfo,
            worktree: WorktreeInfo,
        },
        #[serde(rename = "worktree_opened")]
        WorktreeOpened {
            already_open: bool,
            root_pane: PaneInfo,
            tab: TabInfo,
            workspace: WorkspaceInfo,
            worktree: WorktreeInfo,
        },
        #[serde(rename = "worktree_removed")]
        WorktreeRemoved {
            forced: bool,
            path: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "tab_info")]
        TabInfo { tab: TabInfo },
        #[serde(rename = "tab_created")]
        TabCreated { root_pane: PaneInfo, tab: TabInfo },
        #[serde(rename = "tab_list")]
        TabList { tabs: ::std::vec::Vec<TabInfo> },
        #[serde(rename = "agent_info")]
        AgentInfo { agent: AgentInfo },
        #[serde(rename = "agent_started")]
        AgentStarted {
            agent: AgentInfo,
            argv: ::std::vec::Vec<::std::string::String>,
        },
        #[serde(rename = "agent_prompted")]
        AgentPrompted { agent: AgentInfo },
        #[serde(rename = "agent_list")]
        AgentList { agents: ::std::vec::Vec<AgentInfo> },
        #[serde(rename = "agent_view")]
        AgentView {
            active: bool,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            label: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            source: ::std::option::Option<::std::string::String>,
        },
        #[serde(rename = "pane_info")]
        PaneInfo { pane: PaneInfo },
        #[serde(rename = "pane_list")]
        PaneList { panes: ::std::vec::Vec<PaneInfo> },
        #[serde(rename = "pane_current")]
        PaneCurrent { pane: PaneInfo },
        #[serde(rename = "pane_swap")]
        PaneSwap { swap: PaneSwapResult },
        #[serde(rename = "pane_move")]
        PaneMove { move_result: PaneMoveResult },
        #[serde(rename = "pane_zoom")]
        PaneZoom { zoom: PaneZoomResult },
        #[serde(rename = "pane_layout")]
        PaneLayout { layout: PaneLayoutSnapshot },
        #[serde(rename = "pane_process_info")]
        PaneProcessInfo { process_info: PaneProcessInfo },
        #[serde(rename = "layout_export")]
        LayoutExport { layout: LayoutDescription },
        #[serde(rename = "layout_apply")]
        LayoutApply { layout: LayoutDescription },
        #[serde(rename = "layout_split_ratio_set")]
        LayoutSplitRatioSet { layout: LayoutDescription },
        #[serde(rename = "pane_neighbor")]
        PaneNeighbor { neighbor: PaneNeighborResult },
        #[serde(rename = "pane_edges")]
        PaneEdges { edges: PaneEdgesResult },
        #[serde(rename = "pane_focus_direction")]
        PaneFocusDirection { focus: PaneFocusDirectionResult },
        #[serde(rename = "pane_resize")]
        PaneResize { resize: PaneResizeResult },
        #[serde(rename = "pane_read")]
        PaneRead { read: PaneReadResult },
        #[serde(rename = "pane_selection")]
        PaneSelection {
            pane_id: ::std::string::String,
            text: ::std::string::String,
        },
        #[serde(rename = "pane_copy_motion")]
        PaneCopyMotion {
            content_revision: u64,
            cursor: PaneTextPoint,
            pane_id: ::std::string::String,
        },
        #[serde(rename = "pane_copy_search")]
        PaneCopySearch {
            content_revision: u64,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            current: ::std::option::Option<u32>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            current_global: ::std::option::Option<u64>,
            matches: ::std::vec::Vec<PaneTextRange>,
            pane_id: ::std::string::String,
            total: u64,
        },
        #[serde(rename = "agent_explain")]
        AgentExplain { explain: ::serde_json::Value },
        #[serde(rename = "subscription_started")]
        SubscriptionStarted,
        #[serde(rename = "wait_matched")]
        WaitMatched { event: EventEnvelope },
        #[serde(rename = "output_matched")]
        OutputMatched {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            matched_line: ::std::option::Option<::std::string::String>,
            pane_id: ::std::string::String,
            read: PaneReadResult,
            revision: u64,
        },
        #[serde(rename = "notification_show")]
        NotificationShow {
            reason: NotificationShowReason,
            shown: bool,
        },
        #[serde(rename = "client_window_title")]
        ClientWindowTitle {
            changed: bool,
            reason: ClientWindowTitleReason,
        },
        #[serde(rename = "integration_list")]
        IntegrationList {
            integrations: ::std::vec::Vec<IntegrationInfo>,
        },
        #[serde(rename = "integration_install")]
        IntegrationInstall {
            details: IntegrationInstallResult,
            target: IntegrationTarget,
        },
        #[serde(rename = "integration_uninstall")]
        IntegrationUninstall {
            details: IntegrationUninstallResult,
            target: IntegrationTarget,
        },
        #[serde(rename = "agent_manifest_reload")]
        AgentManifestReload {
            manifests: ::std::vec::Vec<AgentManifestInfo>,
        },
        #[serde(rename = "agent_manifest_status")]
        AgentManifestStatus {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            last_check_unix: ::std::option::Option<u64>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            last_result: ::std::option::Option<::std::string::String>,
            manifests: ::std::vec::Vec<AgentManifestInfo>,
        },
        #[serde(rename = "plugin_linked")]
        PluginLinked { plugin: InstalledPluginInfo },
        #[serde(rename = "plugin_list")]
        PluginList {
            plugins: ::std::vec::Vec<InstalledPluginInfo>,
        },
        #[serde(rename = "plugin_unlinked")]
        PluginUnlinked {
            plugin_id: ::std::string::String,
            removed: bool,
        },
        #[serde(rename = "plugin_enabled")]
        PluginEnabled { plugin: InstalledPluginInfo },
        #[serde(rename = "plugin_disabled")]
        PluginDisabled { plugin: InstalledPluginInfo },
        #[serde(rename = "plugin_action_list")]
        PluginActionList {
            actions: ::std::vec::Vec<PluginActionInfo>,
        },
        #[serde(rename = "plugin_action_invoked")]
        PluginActionInvoked {
            action: PluginActionInfo,
            context: PluginInvocationContext,
            log: PluginCommandLogInfo,
        },
        #[serde(rename = "pane_link_resolved")]
        PaneLinkResolved {
            regions: ::std::vec::Vec<PaneLinkRegion>,
        },
        #[serde(rename = "pane_link_activated")]
        PaneLinkActivated {
            handled: bool,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            url: ::std::option::Option<::std::string::String>,
        },
        #[serde(rename = "plugin_log_list")]
        PluginLogList {
            logs: ::std::vec::Vec<PluginCommandLogInfo>,
        },
        #[serde(rename = "plugin_pane_opened")]
        PluginPaneOpened { plugin_pane: PluginPaneInfo },
        #[serde(rename = "plugin_pane_focused")]
        PluginPaneFocused { plugin_pane: PluginPaneInfo },
        #[serde(rename = "plugin_pane_closed")]
        PluginPaneClosed { pane_id: ::std::string::String },
        #[serde(rename = "config_reload")]
        ConfigReload {
            diagnostics: ::std::vec::Vec<::std::string::String>,
            status: ConfigReloadStatus,
        },
        #[doc = "Acknowledgement for the client-shell surface interest lease. This method is new on the\nendpoint protocol, so its revision-bearing result can establish an activation floor."]
        #[serde(rename = "client_shell_surface_set")]
        ClientShellSurfaceSet {
            active: bool,
            projection_revision: u64,
        },
        #[serde(rename = "ok")]
        Ok,
    }
    #[doc = "`ServerCapabilities`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ServerCapabilities {
        #[serde(default)]
        pub detached_server_daemon: bool,
        #[doc = "Stable client-owned endpoint generation supported by this server."]
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub endpoint_protocol_generation: ::std::option::Option<u32>,
        #[doc = "Whether this server supports endpoint health probes."]
        #[serde(default)]
        pub health_check: bool,
        pub live_handoff: bool,
        #[doc = "Supports connection-scoped `server.ssh_agent.register` on the local JSON API."]
        #[serde(default)]
        pub ssh_agent_registration: bool,
        #[doc = "Whether this server supports explicit client-shell surface interest."]
        #[serde(default)]
        pub surface_interest: bool,
    }
    #[doc = "`SessionSnapshot`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct SessionSnapshot {
        pub agents: ::std::vec::Vec<AgentInfo>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_pane_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_tab_id: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub focused_workspace_id: ::std::option::Option<::std::string::String>,
        pub layouts: ::std::vec::Vec<PaneLayoutSnapshot>,
        pub panes: ::std::vec::Vec<PaneInfo>,
        pub protocol: u32,
        pub tabs: ::std::vec::Vec<TabInfo>,
        pub version: ::std::string::String,
        pub workspaces: ::std::vec::Vec<WorkspaceInfo>,
    }
    #[doc = "`SplitDirection`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum SplitDirection {
        Variant0(SplitDirectionVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for SplitDirection {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<SplitDirectionVariant0> for SplitDirection {
        fn from(value: SplitDirectionVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`SplitDirectionVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum SplitDirectionVariant0 {
        #[serde(rename = "right")]
        Right,
        #[serde(rename = "down")]
        Down,
    }
    impl ::std::fmt::Display for SplitDirectionVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Right => f.write_str("right"),
                Self::Down => f.write_str("down"),
            }
        }
    }
    impl ::std::str::FromStr for SplitDirectionVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "right" => Ok(Self::Right),
                "down" => Ok(Self::Down),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for SplitDirectionVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for SplitDirectionVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`SuccessResponse`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct SuccessResponse {
        pub id: ::std::string::String,
        pub result: ResponseResult,
    }
    #[doc = "`TabInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct TabInfo {
        pub agent_status: AgentStatus,
        pub focused: bool,
        pub label: ::std::string::String,
        pub number: u32,
        pub pane_count: u32,
        pub tab_id: ::std::string::String,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`WorkspaceInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceInfo {
        pub active_tab_id: ::std::string::String,
        pub agent_status: AgentStatus,
        pub focused: bool,
        pub label: ::std::string::String,
        pub number: u32,
        pub pane_count: u32,
        pub tab_count: u32,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub tokens: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        pub workspace_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub worktree: ::std::option::Option<WorkspaceWorktreeInfo>,
    }
    #[doc = "`WorkspaceWorktreeInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceWorktreeInfo {
        pub checkout_path: ::std::string::String,
        pub is_linked_worktree: bool,
        pub repo_key: ::std::string::String,
        pub repo_name: ::std::string::String,
        pub repo_root: ::std::string::String,
    }
    #[doc = "`WorktreeInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorktreeInfo {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub branch: ::std::option::Option<::std::string::String>,
        pub is_bare: bool,
        pub is_detached: bool,
        pub is_linked_worktree: bool,
        pub is_prunable: bool,
        pub label: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub open_workspace_id: ::std::option::Option<::std::string::String>,
        pub path: ::std::string::String,
    }
    #[doc = "`WorktreeSourceInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorktreeSourceInfo {
        pub repo_key: ::std::string::String,
        pub repo_name: ::std::string::String,
        pub repo_root: ::std::string::String,
        pub source_checkout_path: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub source_workspace_id: ::std::option::Option<::std::string::String>,
    }
    #[doc = " Generation of default values for serde."]
    pub mod defaults {
        pub(super) fn installed_plugin_info_source() -> super::PluginSourceInfo {
            super::PluginSourceInfo {
                installed_unix_ms: Default::default(),
                kind: super::PluginSourceKind::Variant0(super::PluginSourceKindVariant0::Local),
                managed_path: Default::default(),
                owner: Default::default(),
                repo: Default::default(),
                requested_ref: Default::default(),
                resolved_commit: Default::default(),
                subdir: Default::default(),
            }
        }
        pub(super) fn plugin_manifest_pane_placement() -> super::PluginPanePlacement {
            super::PluginPanePlacement::Variant0(super::PluginPanePlacementVariant0::Overlay)
        }
        pub(super) fn plugin_source_info_kind() -> super::PluginSourceKind {
            super::PluginSourceKind::Variant0(super::PluginSourceKindVariant0::Local)
        }
    }
    #[doc = " Error types."]
    pub mod error {
        #[doc = r" Error from a `TryFrom` or `FromStr` implementation."]
        pub struct ConversionError(::std::borrow::Cow<'static, str>);
        impl ::std::error::Error for ConversionError {}
        impl ::std::fmt::Display for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Display::fmt(&self.0, f)
            }
        }
        impl ::std::fmt::Debug for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Debug::fmt(&self.0, f)
            }
        }
        impl From<&'static str> for ConversionError {
            fn from(value: &'static str) -> Self {
                Self(value.into())
            }
        }
        impl From<String> for ConversionError {
            fn from(value: String) -> Self {
                Self(value.into())
            }
        }
    }
}

/// Types generated from the `error_response` schema.
pub mod error_response {

    #[doc = "`ErrorBody`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ErrorBody {
        pub code: ::std::string::String,
        pub message: ::std::string::String,
    }
    #[doc = "`ErrorResponse`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ErrorResponse {
        pub error: ErrorBody,
        pub id: ::std::string::String,
    }
}

/// Types generated from the `event` schema.
pub mod event {

    #[doc = "`AgentSessionInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct AgentSessionInfo {
        pub agent: ::std::string::String,
        pub kind: AgentSessionRefKind,
        pub source: ::std::string::String,
        pub value: ::std::string::String,
    }
    #[doc = "`AgentSessionRefKind`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum AgentSessionRefKind {
        Variant0(AgentSessionRefKindVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for AgentSessionRefKind {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<AgentSessionRefKindVariant0> for AgentSessionRefKind {
        fn from(value: AgentSessionRefKindVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`AgentSessionRefKindVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum AgentSessionRefKindVariant0 {
        #[serde(rename = "id")]
        Id,
        #[serde(rename = "path")]
        Path,
    }
    impl ::std::fmt::Display for AgentSessionRefKindVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Id => f.write_str("id"),
                Self::Path => f.write_str("path"),
            }
        }
    }
    impl ::std::str::FromStr for AgentSessionRefKindVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "id" => Ok(Self::Id),
                "path" => Ok(Self::Path),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for AgentSessionRefKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for AgentSessionRefKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`AgentStatus`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum AgentStatus {
        Variant0(AgentStatusVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for AgentStatus {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<AgentStatusVariant0> for AgentStatus {
        fn from(value: AgentStatusVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`AgentStatusVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum AgentStatusVariant0 {
        #[serde(rename = "idle")]
        Idle,
        #[serde(rename = "working")]
        Working,
        #[serde(rename = "blocked")]
        Blocked,
        #[serde(rename = "done")]
        Done,
        #[serde(rename = "unknown")]
        Unknown,
    }
    impl ::std::fmt::Display for AgentStatusVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Idle => f.write_str("idle"),
                Self::Working => f.write_str("working"),
                Self::Blocked => f.write_str("blocked"),
                Self::Done => f.write_str("done"),
                Self::Unknown => f.write_str("unknown"),
            }
        }
    }
    impl ::std::str::FromStr for AgentStatusVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "idle" => Ok(Self::Idle),
                "working" => Ok(Self::Working),
                "blocked" => Ok(Self::Blocked),
                "done" => Ok(Self::Done),
                "unknown" => Ok(Self::Unknown),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for AgentStatusVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for AgentStatusVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`EventData`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(tag = "type")]
    pub enum EventData {
        #[serde(rename = "workspace_created")]
        WorkspaceCreated { workspace: WorkspaceInfo },
        #[serde(rename = "workspace_updated")]
        WorkspaceUpdated { workspace: WorkspaceInfo },
        #[serde(rename = "workspace_metadata_updated")]
        WorkspaceMetadataUpdated { workspace: WorkspaceInfo },
        #[serde(rename = "workspace_closed")]
        WorkspaceClosed {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            workspace: ::std::option::Option<WorkspaceInfo>,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "workspace_renamed")]
        WorkspaceRenamed {
            label: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "workspace_moved")]
        WorkspaceMoved {
            insert_index: u32,
            workspace_id: ::std::string::String,
            workspaces: ::std::vec::Vec<WorkspaceInfo>,
        },
        #[serde(rename = "workspace_reordered")]
        WorkspaceReordered {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            before_workspace_id: ::std::option::Option<::std::string::String>,
            workspace_ids: ::std::vec::Vec<::std::string::String>,
            workspaces: ::std::vec::Vec<WorkspaceInfo>,
        },
        #[serde(rename = "workspace_focused")]
        WorkspaceFocused { workspace_id: ::std::string::String },
        #[serde(rename = "worktree_created")]
        WorktreeCreated {
            workspace: WorkspaceInfo,
            worktree: WorktreeInfo,
        },
        #[serde(rename = "worktree_opened")]
        WorktreeOpened {
            already_open: bool,
            workspace: WorkspaceInfo,
            worktree: WorktreeInfo,
        },
        #[serde(rename = "worktree_removed")]
        WorktreeRemoved {
            forced: bool,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            workspace: ::std::option::Option<WorkspaceInfo>,
            workspace_id: ::std::string::String,
            worktree: WorktreeInfo,
        },
        #[serde(rename = "tab_created")]
        TabCreated { tab: TabInfo },
        #[serde(rename = "tab_closed")]
        TabClosed {
            tab_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "tab_renamed")]
        TabRenamed {
            label: ::std::string::String,
            tab_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "tab_moved")]
        TabMoved {
            insert_index: u32,
            tab_id: ::std::string::String,
            tabs: ::std::vec::Vec<TabInfo>,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "tab_focused")]
        TabFocused {
            tab_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_created")]
        PaneCreated { pane: PaneInfo },
        #[serde(rename = "pane_closed")]
        PaneClosed {
            pane_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_updated")]
        PaneUpdated { pane: PaneInfo },
        #[serde(rename = "pane_focused")]
        PaneFocused {
            pane_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_moved")]
        PaneMoved {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            closed_tab_id: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            closed_workspace_id: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            created_tab: ::std::option::Option<TabInfo>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            created_workspace: ::std::option::Option<WorkspaceInfo>,
            pane: PaneInfo,
            previous_pane_id: ::std::string::String,
            previous_tab_id: ::std::string::String,
            previous_workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_output_changed")]
        PaneOutputChanged {
            pane_id: ::std::string::String,
            revision: u64,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_exited")]
        PaneExited {
            pane_id: ::std::string::String,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_agent_detected")]
        PaneAgentDetected {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            agent: ::std::option::Option<::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            final_status: ::std::option::Option<AgentStatus>,
            pane_id: ::std::string::String,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            released: ::std::option::Option<bool>,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "pane_agent_status_changed")]
        PaneAgentStatusChanged {
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            agent: ::std::option::Option<::std::string::String>,
            agent_status: AgentStatus,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            display_agent: ::std::option::Option<::std::string::String>,
            pane_id: ::std::string::String,
            #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
            state_labels: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
            #[serde(skip_serializing_if = "::std::option::Option::is_none")]
            title: ::std::option::Option<::std::string::String>,
            workspace_id: ::std::string::String,
        },
        #[serde(rename = "layout_updated")]
        LayoutUpdated { layout: PaneLayoutSnapshot },
    }
    #[doc = "`EventEnvelope`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct EventEnvelope {
        pub data: EventData,
        pub event: EventKind,
    }
    #[doc = "`EventKind`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum EventKind {
        Variant0(EventKindVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for EventKind {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<EventKindVariant0> for EventKind {
        fn from(value: EventKindVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`EventKindVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum EventKindVariant0 {
        #[serde(rename = "workspace_created")]
        WorkspaceCreated,
        #[serde(rename = "workspace_updated")]
        WorkspaceUpdated,
        #[serde(rename = "workspace_metadata_updated")]
        WorkspaceMetadataUpdated,
        #[serde(rename = "workspace_closed")]
        WorkspaceClosed,
        #[serde(rename = "workspace_renamed")]
        WorkspaceRenamed,
        #[serde(rename = "workspace_moved")]
        WorkspaceMoved,
        #[serde(rename = "workspace_reordered")]
        WorkspaceReordered,
        #[serde(rename = "workspace_focused")]
        WorkspaceFocused,
        #[serde(rename = "worktree_created")]
        WorktreeCreated,
        #[serde(rename = "worktree_opened")]
        WorktreeOpened,
        #[serde(rename = "worktree_removed")]
        WorktreeRemoved,
        #[serde(rename = "tab_created")]
        TabCreated,
        #[serde(rename = "tab_closed")]
        TabClosed,
        #[serde(rename = "tab_renamed")]
        TabRenamed,
        #[serde(rename = "tab_moved")]
        TabMoved,
        #[serde(rename = "tab_focused")]
        TabFocused,
        #[serde(rename = "pane_created")]
        PaneCreated,
        #[serde(rename = "pane_closed")]
        PaneClosed,
        #[serde(rename = "pane_updated")]
        PaneUpdated,
        #[serde(rename = "pane_focused")]
        PaneFocused,
        #[serde(rename = "pane_moved")]
        PaneMoved,
        #[serde(rename = "pane_output_changed")]
        PaneOutputChanged,
        #[serde(rename = "pane_exited")]
        PaneExited,
        #[serde(rename = "pane_agent_detected")]
        PaneAgentDetected,
        #[serde(rename = "pane_agent_status_changed")]
        PaneAgentStatusChanged,
        #[serde(rename = "layout_updated")]
        LayoutUpdated,
    }
    impl ::std::fmt::Display for EventKindVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::WorkspaceCreated => f.write_str("workspace_created"),
                Self::WorkspaceUpdated => f.write_str("workspace_updated"),
                Self::WorkspaceMetadataUpdated => f.write_str("workspace_metadata_updated"),
                Self::WorkspaceClosed => f.write_str("workspace_closed"),
                Self::WorkspaceRenamed => f.write_str("workspace_renamed"),
                Self::WorkspaceMoved => f.write_str("workspace_moved"),
                Self::WorkspaceReordered => f.write_str("workspace_reordered"),
                Self::WorkspaceFocused => f.write_str("workspace_focused"),
                Self::WorktreeCreated => f.write_str("worktree_created"),
                Self::WorktreeOpened => f.write_str("worktree_opened"),
                Self::WorktreeRemoved => f.write_str("worktree_removed"),
                Self::TabCreated => f.write_str("tab_created"),
                Self::TabClosed => f.write_str("tab_closed"),
                Self::TabRenamed => f.write_str("tab_renamed"),
                Self::TabMoved => f.write_str("tab_moved"),
                Self::TabFocused => f.write_str("tab_focused"),
                Self::PaneCreated => f.write_str("pane_created"),
                Self::PaneClosed => f.write_str("pane_closed"),
                Self::PaneUpdated => f.write_str("pane_updated"),
                Self::PaneFocused => f.write_str("pane_focused"),
                Self::PaneMoved => f.write_str("pane_moved"),
                Self::PaneOutputChanged => f.write_str("pane_output_changed"),
                Self::PaneExited => f.write_str("pane_exited"),
                Self::PaneAgentDetected => f.write_str("pane_agent_detected"),
                Self::PaneAgentStatusChanged => f.write_str("pane_agent_status_changed"),
                Self::LayoutUpdated => f.write_str("layout_updated"),
            }
        }
    }
    impl ::std::str::FromStr for EventKindVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "workspace_created" => Ok(Self::WorkspaceCreated),
                "workspace_updated" => Ok(Self::WorkspaceUpdated),
                "workspace_metadata_updated" => Ok(Self::WorkspaceMetadataUpdated),
                "workspace_closed" => Ok(Self::WorkspaceClosed),
                "workspace_renamed" => Ok(Self::WorkspaceRenamed),
                "workspace_moved" => Ok(Self::WorkspaceMoved),
                "workspace_reordered" => Ok(Self::WorkspaceReordered),
                "workspace_focused" => Ok(Self::WorkspaceFocused),
                "worktree_created" => Ok(Self::WorktreeCreated),
                "worktree_opened" => Ok(Self::WorktreeOpened),
                "worktree_removed" => Ok(Self::WorktreeRemoved),
                "tab_created" => Ok(Self::TabCreated),
                "tab_closed" => Ok(Self::TabClosed),
                "tab_renamed" => Ok(Self::TabRenamed),
                "tab_moved" => Ok(Self::TabMoved),
                "tab_focused" => Ok(Self::TabFocused),
                "pane_created" => Ok(Self::PaneCreated),
                "pane_closed" => Ok(Self::PaneClosed),
                "pane_updated" => Ok(Self::PaneUpdated),
                "pane_focused" => Ok(Self::PaneFocused),
                "pane_moved" => Ok(Self::PaneMoved),
                "pane_output_changed" => Ok(Self::PaneOutputChanged),
                "pane_exited" => Ok(Self::PaneExited),
                "pane_agent_detected" => Ok(Self::PaneAgentDetected),
                "pane_agent_status_changed" => Ok(Self::PaneAgentStatusChanged),
                "layout_updated" => Ok(Self::LayoutUpdated),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for EventKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for EventKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneInfo {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent_session: ::std::option::Option<AgentSessionInfo>,
        pub agent_status: AgentStatus,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub display_agent: ::std::option::Option<::std::string::String>,
        pub focused: bool,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub foreground_cwd: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub label: ::std::option::Option<::std::string::String>,
        pub pane_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub restore_error: ::std::option::Option<::std::string::String>,
        pub revision: u64,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub scroll: ::std::option::Option<PaneScrollInfo>,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub state_labels: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        pub tab_id: ::std::string::String,
        pub terminal_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub terminal_title: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub terminal_title_stripped: ::std::option::Option<::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub title: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub tokens: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`PaneLayoutPane`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneLayoutPane {
        pub focused: bool,
        pub pane_id: ::std::string::String,
        pub rect: PaneLayoutRect,
    }
    #[doc = "`PaneLayoutRect`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneLayoutRect {
        pub height: u16,
        pub width: u16,
        pub x: u16,
        pub y: u16,
    }
    #[doc = "`PaneLayoutSnapshot`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneLayoutSnapshot {
        pub area: PaneLayoutRect,
        pub focused_pane_id: ::std::string::String,
        pub panes: ::std::vec::Vec<PaneLayoutPane>,
        pub splits: ::std::vec::Vec<PaneLayoutSplit>,
        pub tab_id: ::std::string::String,
        pub workspace_id: ::std::string::String,
        pub zoomed: bool,
    }
    #[doc = "`PaneLayoutSplit`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneLayoutSplit {
        pub direction: SplitDirection,
        pub id: ::std::string::String,
        pub ratio: f32,
        pub rect: PaneLayoutRect,
    }
    #[doc = "`PaneScrollInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneScrollInfo {
        pub max_offset_from_bottom: u64,
        pub offset_from_bottom: u64,
        pub viewport_rows: u64,
    }
    #[doc = "`SplitDirection`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum SplitDirection {
        Variant0(SplitDirectionVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for SplitDirection {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<SplitDirectionVariant0> for SplitDirection {
        fn from(value: SplitDirectionVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`SplitDirectionVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum SplitDirectionVariant0 {
        #[serde(rename = "right")]
        Right,
        #[serde(rename = "down")]
        Down,
    }
    impl ::std::fmt::Display for SplitDirectionVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Right => f.write_str("right"),
                Self::Down => f.write_str("down"),
            }
        }
    }
    impl ::std::str::FromStr for SplitDirectionVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "right" => Ok(Self::Right),
                "down" => Ok(Self::Down),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for SplitDirectionVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for SplitDirectionVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`TabInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct TabInfo {
        pub agent_status: AgentStatus,
        pub focused: bool,
        pub label: ::std::string::String,
        pub number: u32,
        pub pane_count: u32,
        pub tab_id: ::std::string::String,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`WorkspaceInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceInfo {
        pub active_tab_id: ::std::string::String,
        pub agent_status: AgentStatus,
        pub focused: bool,
        pub label: ::std::string::String,
        pub number: u32,
        pub pane_count: u32,
        pub tab_count: u32,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub tokens: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        pub workspace_id: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub worktree: ::std::option::Option<WorkspaceWorktreeInfo>,
    }
    #[doc = "`WorkspaceWorktreeInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorkspaceWorktreeInfo {
        pub checkout_path: ::std::string::String,
        pub is_linked_worktree: bool,
        pub repo_key: ::std::string::String,
        pub repo_name: ::std::string::String,
        pub repo_root: ::std::string::String,
    }
    #[doc = "`WorktreeInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WorktreeInfo {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub branch: ::std::option::Option<::std::string::String>,
        pub is_bare: bool,
        pub is_detached: bool,
        pub is_linked_worktree: bool,
        pub is_prunable: bool,
        pub label: ::std::string::String,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub open_workspace_id: ::std::option::Option<::std::string::String>,
        pub path: ::std::string::String,
    }
    #[doc = " Error types."]
    pub mod error {
        #[doc = r" Error from a `TryFrom` or `FromStr` implementation."]
        pub struct ConversionError(::std::borrow::Cow<'static, str>);
        impl ::std::error::Error for ConversionError {}
        impl ::std::fmt::Display for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Display::fmt(&self.0, f)
            }
        }
        impl ::std::fmt::Debug for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Debug::fmt(&self.0, f)
            }
        }
        impl From<&'static str> for ConversionError {
            fn from(value: &'static str) -> Self {
                Self(value.into())
            }
        }
        impl From<String> for ConversionError {
            fn from(value: String) -> Self {
                Self(value.into())
            }
        }
    }
}

/// Types generated from the `subscription_event` schema.
pub mod subscription_event {

    #[doc = "`AgentStatus`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum AgentStatus {
        Variant0(AgentStatusVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for AgentStatus {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<AgentStatusVariant0> for AgentStatus {
        fn from(value: AgentStatusVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`AgentStatusVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum AgentStatusVariant0 {
        #[serde(rename = "idle")]
        Idle,
        #[serde(rename = "working")]
        Working,
        #[serde(rename = "blocked")]
        Blocked,
        #[serde(rename = "done")]
        Done,
        #[serde(rename = "unknown")]
        Unknown,
    }
    impl ::std::fmt::Display for AgentStatusVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Idle => f.write_str("idle"),
                Self::Working => f.write_str("working"),
                Self::Blocked => f.write_str("blocked"),
                Self::Done => f.write_str("done"),
                Self::Unknown => f.write_str("unknown"),
            }
        }
    }
    impl ::std::str::FromStr for AgentStatusVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "idle" => Ok(Self::Idle),
                "working" => Ok(Self::Working),
                "blocked" => Ok(Self::Blocked),
                "done" => Ok(Self::Done),
                "unknown" => Ok(Self::Unknown),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for AgentStatusVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for AgentStatusVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`PaneAgentStatusChangedEvent`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneAgentStatusChangedEvent {
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub agent: ::std::option::Option<::std::string::String>,
        pub agent_status: AgentStatus,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub display_agent: ::std::option::Option<::std::string::String>,
        pub pane_id: ::std::string::String,
        #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
        pub state_labels: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        #[serde(skip_serializing_if = "::std::option::Option::is_none")]
        pub title: ::std::option::Option<::std::string::String>,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`PaneOutputMatchedEvent`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneOutputMatchedEvent {
        pub matched_line: ::std::string::String,
        pub pane_id: ::std::string::String,
        pub read: PaneReadResult,
    }
    #[doc = "`PaneReadResult`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneReadResult {
        pub format: ReadFormat,
        pub pane_id: ::std::string::String,
        pub revision: u64,
        pub source: ReadSource,
        pub tab_id: ::std::string::String,
        pub text: ::std::string::String,
        pub truncated: bool,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`PaneScrollChangedEvent`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneScrollChangedEvent {
        pub pane_id: ::std::string::String,
        pub scroll: PaneScrollInfo,
        pub workspace_id: ::std::string::String,
    }
    #[doc = "`PaneScrollInfo`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PaneScrollInfo {
        pub max_offset_from_bottom: u64,
        pub offset_from_bottom: u64,
        pub viewport_rows: u64,
    }
    #[doc = "`ReadFormat`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum ReadFormat {
        Variant0(ReadFormatVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for ReadFormat {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<ReadFormatVariant0> for ReadFormat {
        fn from(value: ReadFormatVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`ReadFormatVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum ReadFormatVariant0 {
        #[serde(rename = "text")]
        Text,
        #[serde(rename = "ansi")]
        Ansi,
    }
    impl ::std::fmt::Display for ReadFormatVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Text => f.write_str("text"),
                Self::Ansi => f.write_str("ansi"),
            }
        }
    }
    impl ::std::str::FromStr for ReadFormatVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "text" => Ok(Self::Text),
                "ansi" => Ok(Self::Ansi),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for ReadFormatVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for ReadFormatVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`ReadSource`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum ReadSource {
        Variant0(ReadSourceVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for ReadSource {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<ReadSourceVariant0> for ReadSource {
        fn from(value: ReadSourceVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`ReadSourceVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum ReadSourceVariant0 {
        #[serde(rename = "visible")]
        Visible,
        #[serde(rename = "recent")]
        Recent,
        #[serde(rename = "recent_unwrapped")]
        RecentUnwrapped,
        #[serde(rename = "detection")]
        Detection,
    }
    impl ::std::fmt::Display for ReadSourceVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::Visible => f.write_str("visible"),
                Self::Recent => f.write_str("recent"),
                Self::RecentUnwrapped => f.write_str("recent_unwrapped"),
                Self::Detection => f.write_str("detection"),
            }
        }
    }
    impl ::std::str::FromStr for ReadSourceVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "visible" => Ok(Self::Visible),
                "recent" => Ok(Self::Recent),
                "recent_unwrapped" => Ok(Self::RecentUnwrapped),
                "detection" => Ok(Self::Detection),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for ReadSourceVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for ReadSourceVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = "`SubscriptionEventData`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum SubscriptionEventData {
        OutputMatchedEvent(PaneOutputMatchedEvent),
        AgentStatusChangedEvent(PaneAgentStatusChangedEvent),
        ScrollChangedEvent(PaneScrollChangedEvent),
    }
    impl ::std::convert::From<PaneOutputMatchedEvent> for SubscriptionEventData {
        fn from(value: PaneOutputMatchedEvent) -> Self {
            Self::OutputMatchedEvent(value)
        }
    }
    impl ::std::convert::From<PaneAgentStatusChangedEvent> for SubscriptionEventData {
        fn from(value: PaneAgentStatusChangedEvent) -> Self {
            Self::AgentStatusChangedEvent(value)
        }
    }
    impl ::std::convert::From<PaneScrollChangedEvent> for SubscriptionEventData {
        fn from(value: PaneScrollChangedEvent) -> Self {
            Self::ScrollChangedEvent(value)
        }
    }
    #[doc = "`SubscriptionEventEnvelope`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct SubscriptionEventEnvelope {
        pub data: SubscriptionEventData,
        pub event: SubscriptionEventKind,
    }
    #[doc = "`SubscriptionEventKind`"]
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum SubscriptionEventKind {
        Variant0(SubscriptionEventKindVariant0),
        Variant1(::std::string::String),
    }
    impl ::std::fmt::Display for SubscriptionEventKind {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match self {
                Self::Variant0(x) => x.fmt(f),
                Self::Variant1(x) => x.fmt(f),
            }
        }
    }
    impl ::std::convert::From<SubscriptionEventKindVariant0> for SubscriptionEventKind {
        fn from(value: SubscriptionEventKindVariant0) -> Self {
            Self::Variant0(value)
        }
    }
    #[doc = "`SubscriptionEventKindVariant0`"]
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Copy,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    pub enum SubscriptionEventKindVariant0 {
        #[serde(rename = "pane.output_matched")]
        PaneOutputMatched,
        #[serde(rename = "pane.agent_status_changed")]
        PaneAgentStatusChanged,
        #[serde(rename = "pane.scroll_changed")]
        PaneScrollChanged,
    }
    impl ::std::fmt::Display for SubscriptionEventKindVariant0 {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            match *self {
                Self::PaneOutputMatched => f.write_str("pane.output_matched"),
                Self::PaneAgentStatusChanged => f.write_str("pane.agent_status_changed"),
                Self::PaneScrollChanged => f.write_str("pane.scroll_changed"),
            }
        }
    }
    impl ::std::str::FromStr for SubscriptionEventKindVariant0 {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            match value {
                "pane.output_matched" => Ok(Self::PaneOutputMatched),
                "pane.agent_status_changed" => Ok(Self::PaneAgentStatusChanged),
                "pane.scroll_changed" => Ok(Self::PaneScrollChanged),
                _ => Err("invalid value".into()),
            }
        }
    }
    impl ::std::convert::TryFrom<&str> for SubscriptionEventKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    impl ::std::convert::TryFrom<::std::string::String> for SubscriptionEventKindVariant0 {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }
    #[doc = " Error types."]
    pub mod error {
        #[doc = r" Error from a `TryFrom` or `FromStr` implementation."]
        pub struct ConversionError(::std::borrow::Cow<'static, str>);
        impl ::std::error::Error for ConversionError {}
        impl ::std::fmt::Display for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Display::fmt(&self.0, f)
            }
        }
        impl ::std::fmt::Debug for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Debug::fmt(&self.0, f)
            }
        }
        impl From<&'static str> for ConversionError {
            fn from(value: &'static str) -> Self {
                Self(value.into())
            }
        }
        impl From<String> for ConversionError {
            fn from(value: String) -> Self {
                Self(value.into())
            }
        }
    }
}
