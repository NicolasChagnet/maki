use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use maki_storage::paths;
use strum::{Display, EnumIter, EnumString, IntoEnumIterator};

use crate::template::Vars;

pub trait ValidNames: IntoEnumIterator + std::fmt::Display {
    fn valid_names() -> String {
        Self::iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub const SYSTEM_PROMPT: &str = include_str!("prompts/system.md");
pub const PLAN_PROMPT: &str = include_str!("prompts/plan.md");
pub const RESEARCH_PROMPT: &str = include_str!("prompts/research.md");
pub const GENERAL_PROMPT: &str = include_str!("prompts/general.md");
pub const COMPACTION_SYSTEM: &str = include_str!("prompts/compaction.md");
pub const COMPACTION_USER: &str = include_str!("prompts/compaction_user.md");

pub const DEFAULT_IDENTITY: &str = r#"You are Maki, an interactive CLI coding agent. Use the tools available to assist the user with software engineering tasks. Complete tasks successfully while minimizing token usage and tool calls to avoid context bloat.

You must NEVER generate or guess URLs unless they are for helping the user with programming."#;

pub const DEFAULT_TONE: &str = r#"- Be concise. Your output is displayed on a CLI rendered in monospace. Use GitHub-flavored markdown.
- Only use emojis if explicitly requested.
- Do not add comments to code unless asked.
- Output text to communicate with the user; all text you output outside of tool use is displayed to the user. Only use tools to complete tasks. NEVER use bash echo or other command-line tools to communicate thoughts, explanations, diagrams, or instructions to the user. Output all communication directly in your response text instead.
- NEVER create files unless absolutely necessary. ALWAYS prefer editing existing files."#;

const NATIVE_EFFICIENT_TOOLS: &[&str] = &["batch", "code_execution", "task"];
const INSTRUCTIONS_MARKER: &str = "{{instructions}}";

/// Singleton: alphabetically last plugin wins, discarding all prior content
/// and built-in defaults.  Used for slots with opinionated defaults where
/// multiple contributors would conflict (identity, tone).
///
/// Aggregate: all entries are joined.  Used for genuinely additive slots
/// where multiple plugins contributing is the point (tool usage hints,
/// efficient tools, after-instructions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, Display)]
#[strum(serialize_all = "snake_case")]
pub enum SlotKind {
    Singleton,
    Aggregate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, Display, EnumIter)]
#[strum(serialize_all = "snake_case")]
pub enum Slot {
    Identity,
    Tone,
    ToolUsage,
    EfficientTools,
    Conventions,
    AfterInstructions,
}

impl Slot {
    fn marker(self) -> &'static str {
        match self {
            Slot::Identity => "{{identity}}",
            Slot::Tone => "{{tone}}",
            Slot::ToolUsage => "{{tool_usage}}",
            Slot::EfficientTools => "{{efficient_tools}}",
            Slot::Conventions => "{{conventions}}",
            Slot::AfterInstructions => "{{after_instructions}}",
        }
    }

    pub fn kind(self) -> SlotKind {
        match self {
            Slot::Identity | Slot::Tone => SlotKind::Singleton,
            Slot::ToolUsage
            | Slot::EfficientTools
            | Slot::Conventions
            | Slot::AfterInstructions => SlotKind::Aggregate,
        }
    }

    /// Built-in default content for singleton slots.  When no plugin
    /// registers content for a singleton slot, the default is used.
    /// Aggregate slots have no default (the template carries the static
    /// text around the marker).
    pub fn default_content(self) -> Option<&'static str> {
        match self {
            Slot::Identity => Some(DEFAULT_IDENTITY),
            Slot::Tone => Some(DEFAULT_TONE),
            _ => None,
        }
    }

    pub fn names_for_kind(kind: SlotKind) -> String {
        Self::iter()
            .filter(|s| s.kind() == kind)
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, Display, EnumIter)]
#[strum(serialize_all = "snake_case")]
pub enum PromptId {
    System,
    Research,
    General,
}

impl PromptId {
    pub const ALL: &[PromptId] = &[PromptId::System, PromptId::Research, PromptId::General];
}

impl ValidNames for Slot {}
impl ValidNames for PromptId {}

pub struct SlotEntry {
    pub plugin: Arc<str>,
    pub content: String,
}

#[derive(Default)]
pub struct ResolvedSlots {
    entries: HashMap<(PromptId, Slot), Vec<SlotEntry>>,
}

impl ResolvedSlots {
    pub fn get(&self, prompt: PromptId, slot: Slot) -> &[SlotEntry] {
        self.entries
            .get(&(prompt, slot))
            .map(|v| v.as_slice())
            .unwrap_or_default()
    }

    pub fn insert(&mut self, prompt: PromptId, slot: Slot, entry: SlotEntry) {
        self.entries.entry((prompt, slot)).or_default().push(entry);
    }
}

impl PromptId {
    fn template(self) -> &'static str {
        match self {
            PromptId::System => SYSTEM_PROMPT,
            PromptId::Research => RESEARCH_PROMPT,
            PromptId::General => GENERAL_PROMPT,
        }
    }

    /// A slot exists for this prompt iff its marker is present in the template.
    /// Markers that are absent get no content (and we warn at collection time
    /// when a plugin targets them explicitly).
    pub fn has_slot(self, slot: Slot) -> bool {
        self.template().contains(slot.marker())
    }
}

fn render_slot(slots: &ResolvedSlots, prompt: PromptId, slot: Slot) -> String {
    if slot == Slot::EfficientTools {
        return render_efficient_tools(slots, prompt);
    }
    let entries = slots.get(prompt, slot);
    match slot.kind() {
        SlotKind::Singleton => {
            if let Some(last) = entries.last() {
                last.content.clone()
            } else if let Some(default) = slot.default_content() {
                default.to_string()
            } else {
                String::new()
            }
        }
        // Aggregate slots have no built-in defaults; content comes entirely from plugins.
        SlotKind::Aggregate => {
            let mut parts = Vec::new();
            for entry in entries {
                parts.push(entry.content.as_str());
            }
            parts.join("\n")
        }
    }
}

fn render_efficient_tools(slots: &ResolvedSlots, prompt: PromptId) -> String {
    let extras = slots.get(prompt, Slot::EfficientTools);
    let names = NATIVE_EFFICIENT_TOOLS
        .iter()
        .copied()
        .chain(extras.iter().map(|e| e.content.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Most efficient tools: {names}.")
}

/// Fill each `{{slot}}` marker in the template with its rendered content and
/// drop the project instructions (AGENTS.md and friends) into `{{instructions}}`.
///
/// Uses only the in-code defaults; see [`assemble_with_overrides`] for the
/// user-overridable path.
pub fn assemble(id: PromptId, slots: &ResolvedSlots, instructions: &str) -> String {
    assemble_with_overrides(id, slots, instructions, &PromptOverrides::default(), None)
}

const PLAN_MODE_MARKER: &str = "{{plan_mode}}";

/// Same as `assemble` but honors user overrides: the template for `id`, the
/// singleton `{{identity}}` / `{{tone}}` slots, and the `{{plan_mode}}` marker
/// (when `plan_path` is `Some`) all come from user files when present.
pub fn assemble_with_overrides(
    id: PromptId,
    slots: &ResolvedSlots,
    instructions: &str,
    overrides: &PromptOverrides,
    plan_path: Option<&Path>,
) -> String {
    let template = match id {
        PromptId::System => overrides.system.as_deref().unwrap_or(SYSTEM_PROMPT),
        PromptId::Research => overrides.research.as_deref().unwrap_or(RESEARCH_PROMPT),
        PromptId::General => overrides.general.as_deref().unwrap_or(GENERAL_PROMPT),
    };
    let mut out = template.to_string();
    for slot in Slot::iter() {
        out = fill_marker(&out, slot.marker(), &render_slot(slots, id, slot));
    }
    out = out.replace(INSTRUCTIONS_MARKER, instructions);

    let plan_body = plan_path.map(|path| {
        let body = overrides.plan.as_deref().unwrap_or(PLAN_PROMPT);
        let plan_vars = Vars::new().set("{plan_path}", path.display().to_string());
        plan_vars.apply(body).into_owned()
    });
    let plan_body = plan_body.unwrap_or_default();
    out = fill_marker(&out, PLAN_MODE_MARKER, &plan_body);
    out
}

/// Replace a slot marker with its content. When the content is empty, also drop
/// the marker's own line (the trailing newline) so empty slots leave no blank
/// gap, without touching any other whitespace in the prompt.
fn fill_marker(template: &str, marker: &str, content: &str) -> String {
    if content.is_empty() {
        return template
            .replace(&format!("{marker}\n"), "")
            .replace(marker, "");
    }
    template.replace(marker, content)
}

/// User-overridable system-prompt templates loaded from the config dir.
/// Each `Option` is `Some` once a user `prompts/<name>.md` file exists (the
/// loader seeds missing canonical files with the shipped default); `None`
/// means the in-code default applies.
#[derive(Debug, Clone, Default)]
pub struct PromptOverrides {
    pub system: Option<String>,
    pub plan: Option<String>,
    pub research: Option<String>,
    pub general: Option<String>,
    pub compaction_system: Option<String>,
    pub compaction_user: Option<String>,
}

type OverrideField = fn(&mut PromptOverrides) -> &mut Option<String>;
type OverrideDef = (&'static str, &'static str, OverrideField);

/// `(file leaf name, in-code default, target field)` for load resolution.
const PROM_DEFS: &[OverrideDef] = &[
    ("system", SYSTEM_PROMPT, |o| &mut o.system),
    ("plan", PLAN_PROMPT, |o| &mut o.plan),
    ("research", RESEARCH_PROMPT, |o| &mut o.research),
    ("general", GENERAL_PROMPT, |o| &mut o.general),
    ("compaction_system", COMPACTION_SYSTEM, |o| {
        &mut o.compaction_system
    }),
    ("compaction_user", COMPACTION_USER, |o| {
        &mut o.compaction_user
    }),
];

impl PromptOverrides {
    /// Load once per agent. Uses the real `home`/config dirs.
    pub fn load() -> Self {
        let home = paths::home();
        let xdg_config = paths::config_dir().ok();
        Self::load_with_dirs(home.as_deref(), xdg_config.as_deref())
    }

    /// Resolution per file: legacy `~/.maki/prompts/<name>.md` first, then the
    /// config-dir `prompts/<name>.md`, matching how AGENTS.md is resolved. The
    /// config-dir file is seeded from the default when missing; existing files
    /// are never overwritten. Failures degrade to `None` with a `tracing::warn!`.
    pub(crate) fn load_with_dirs(home: Option<&Path>, xdg_config: Option<&Path>) -> Self {
        let mut out = Self::default();
        for &(name, default, field) in PROM_DEFS {
            *field(&mut out) = load_one(home, xdg_config, name, default);
        }
        out
    }
}

fn load_one(
    home: Option<&Path>,
    xdg_config: Option<&Path>,
    name: &str,
    default: &str,
) -> Option<String> {
    if let Some(xdg) = xdg_config {
        let canonical = xdg.join("prompts").join(format!("{name}.md"));
        if !canonical.exists() {
            let parent = canonical.parent()?;
            if let Err(e) = fs::create_dir_all(parent) {
                tracing::warn!(%name, error = %e, "prompt override init: cannot create prompts dir");
            } else if let Err(e) = maki_storage::atomic_write(&canonical, default.as_bytes()) {
                tracing::warn!(%name, error = %e, "prompt override init: cannot write default");
            }
        }
    }

    // First existing file wins, in `user_config_dirs` order (legacy `~/.maki`
    // before XDG), matching the AGENTS.md resolution convention.
    for path in paths::user_config_dirs(home, xdg_config, &format!("prompts/{name}.md")) {
        if !path.exists() {
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(content) => {
                tracing::debug!(%name, path = %path.display(), "loaded prompt override");
                return Some(content);
            }
            Err(e) => {
                tracing::warn!(%name, path = %path.display(), error = %e, "prompt override unreadable")
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    const NATIVE_EFFICIENT_LINE: &str = "Most efficient tools: batch, code_execution, task";

    fn slots(prompt: PromptId, entries: &[(Slot, &str)]) -> ResolvedSlots {
        let mut slots = ResolvedSlots::default();
        for &(slot, content) in entries {
            slots.insert(
                prompt,
                slot,
                SlotEntry {
                    plugin: Arc::from("p"),
                    content: content.into(),
                },
            );
        }
        slots
    }

    fn at(out: &str, needle: &str) -> usize {
        out.find(needle)
            .unwrap_or_else(|| panic!("missing: {needle}"))
    }

    #[test]
    fn empty_slots_emit_template_and_native_efficient_line() {
        let out = assemble(PromptId::System, &ResolvedSlots::default(), "");
        assert!(out.starts_with("You are Maki"));
        assert!(
            !out.contains("{{"),
            "unfilled marker left in output:\n{out}"
        );
        assert!(out.contains(&format!("{NATIVE_EFFICIENT_LINE}.")));
    }

    /// One test to pin the whole System layout: every slot shows up, in order,
    /// around the instructions. Covers presence and ordering for all of them.
    #[test]
    fn system_sections_land_in_layout_order() {
        let s = slots(
            PromptId::System,
            &[
                (Slot::ToolUsage, "TOOL_USAGE"),
                (Slot::EfficientTools, "EXTRA_TOOL"),
                (Slot::Conventions, "CONVENTIONS"),
                (Slot::AfterInstructions, "AFTER"),
            ],
        );
        let out = assemble(PromptId::System, &s, "INSTR");
        let positions = ["TOOL_USAGE", "EXTRA_TOOL", "CONVENTIONS", "INSTR", "AFTER"]
            .map(|needle| at(&out, needle));
        assert!(
            positions.is_sorted(),
            "sections out of layout order ({positions:?}):\n{out}"
        );
    }

    /// Regression: a `tool_usage` hint must land inside the `# Tool usage`
    /// section, not be appended after the rest of the prompt.
    #[test]
    fn tool_usage_hint_lands_inside_tool_usage_section() {
        const HINT: &str = "- HINT_LINE";
        let s = slots(PromptId::System, &[(Slot::ToolUsage, HINT)]);
        let out = assemble(PromptId::System, &s, "");
        let hint = at(&out, HINT);
        assert!(
            at(&out, "# Tool usage") < hint,
            "hint before its section:\n{out}"
        );
        assert!(
            hint < at(&out, "# Conventions"),
            "hint leaked past section:\n{out}"
        );
    }

    #[test]
    fn efficient_tools_extras_join_native_list() {
        let s = slots(
            PromptId::System,
            &[
                (Slot::EfficientTools, "index"),
                (Slot::EfficientTools, "foo"),
            ],
        );
        let out = assemble(PromptId::System, &s, "");
        assert!(out.contains(&format!("{NATIVE_EFFICIENT_LINE}, index, foo.")));
    }

    #[test]
    fn same_slot_preserves_insertion_order() {
        let s = slots(
            PromptId::System,
            &[(Slot::ToolUsage, "FIRST"), (Slot::ToolUsage, "SECOND")],
        );
        let out = assemble(PromptId::System, &s, "");
        assert!(at(&out, "FIRST") < at(&out, "SECOND"));
    }

    /// Only System carries AfterInstructions, so the same content shows up there
    /// but never leaks into the subagent prompts.
    #[test]
    fn after_instructions_only_reaches_system() {
        let mut s = ResolvedSlots::default();
        for &pid in PromptId::ALL {
            s.insert(
                pid,
                Slot::AfterInstructions,
                SlotEntry {
                    plugin: Arc::from("p"),
                    content: "AFTER".into(),
                },
            );
        }
        assert!(assemble(PromptId::System, &s, "").contains("AFTER"));
        assert!(!assemble(PromptId::Research, &s, "").contains("AFTER"));
        assert!(!assemble(PromptId::General, &s, "").contains("AFTER"));
    }

    #[test]
    fn research_drops_conventions_but_keeps_efficient_extras() {
        let s = slots(
            PromptId::Research,
            &[
                (Slot::Conventions, "DROPPED"),
                (Slot::EfficientTools, "EXTRA"),
            ],
        );
        let out = assemble(PromptId::Research, &s, "");
        assert!(!out.contains("DROPPED"));
        assert!(out.contains(&format!("{NATIVE_EFFICIENT_LINE}, EXTRA.")));
    }

    #[test_case(PromptId::System, Slot::ToolUsage, true ; "system_tool_usage")]
    #[test_case(PromptId::System, Slot::EfficientTools, true ; "system_efficient")]
    #[test_case(PromptId::System, Slot::Conventions, true ; "system_conventions")]
    #[test_case(PromptId::System, Slot::AfterInstructions, true ; "system_after")]
    #[test_case(PromptId::System, Slot::Identity, true ; "system_identity")]
    #[test_case(PromptId::System, Slot::Tone, true ; "system_tone")]
    #[test_case(PromptId::Research, Slot::Conventions, false ; "research_no_conventions")]
    #[test_case(PromptId::Research, Slot::AfterInstructions, false ; "research_no_after")]
    #[test_case(PromptId::Research, Slot::Identity, false ; "research_no_identity")]
    #[test_case(PromptId::Research, Slot::Tone, false ; "research_no_tone")]
    #[test_case(PromptId::General, Slot::AfterInstructions, false ; "general_no_after")]
    #[test_case(PromptId::General, Slot::Identity, false ; "general_no_identity")]
    #[test_case(PromptId::General, Slot::Tone, false ; "general_no_tone")]
    fn has_slot(prompt: PromptId, slot: Slot, expected: bool) {
        assert_eq!(prompt.has_slot(slot), expected);
    }

    #[test_case("after_instructions", Some(Slot::AfterInstructions) ; "valid_slot")]
    #[test_case("tool_usagee", None ; "typo_slot")]
    #[test_case("identity", Some(Slot::Identity) ; "identity_slot")]
    #[test_case("tone", Some(Slot::Tone) ; "tone_slot")]
    fn slot_parse_is_plugin_contract(input: &str, expected: Option<Slot>) {
        assert_eq!(input.parse::<Slot>().ok(), expected);
    }

    #[test_case("system", Some(PromptId::System) ; "valid_prompt")]
    #[test_case("systm", None ; "typo_prompt")]
    fn prompt_parse_is_plugin_contract(input: &str, expected: Option<PromptId>) {
        assert_eq!(input.parse::<PromptId>().ok(), expected);
    }

    #[test_case(Slot::Identity, SlotKind::Singleton ; "identity_singleton")]
    #[test_case(Slot::Tone, SlotKind::Singleton ; "tone_singleton")]
    #[test_case(Slot::Conventions, SlotKind::Aggregate ; "conventions_aggregate")]
    #[test_case(Slot::ToolUsage, SlotKind::Aggregate ; "tool_usage_aggregate")]
    #[test_case(Slot::EfficientTools, SlotKind::Aggregate ; "efficient_aggregate")]
    #[test_case(Slot::AfterInstructions, SlotKind::Aggregate ; "after_aggregate")]
    fn slot_kind_matches_expectations(slot: Slot, expected: SlotKind) {
        assert_eq!(slot.kind(), expected);
    }

    #[test]
    fn singleton_default_used_when_empty() {
        let out = assemble(PromptId::System, &ResolvedSlots::default(), "");
        assert!(out.starts_with("You are Maki"));
    }

    #[test]
    fn singleton_entry_replaces_default() {
        let mut s = ResolvedSlots::default();
        s.insert(
            PromptId::System,
            Slot::Identity,
            SlotEntry {
                plugin: Arc::from("user"),
                content: "Custom identity".into(),
            },
        );
        let out = assemble(PromptId::System, &s, "");
        assert!(out.contains("Custom identity"));
        assert!(!out.contains("You are Maki"));
    }

    #[test]
    fn singleton_last_entry_wins() {
        let mut s = ResolvedSlots::default();
        s.insert(
            PromptId::System,
            Slot::Identity,
            SlotEntry {
                plugin: Arc::from("first"),
                content: "FIRST".into(),
            },
        );
        s.insert(
            PromptId::System,
            Slot::Identity,
            SlotEntry {
                plugin: Arc::from("second"),
                content: "SECOND".into(),
            },
        );
        let out = assemble(PromptId::System, &s, "");
        assert!(out.contains("SECOND"));
        assert!(!out.contains("FIRST"));
        assert!(!out.contains("You are Maki"));
    }

    #[test]
    fn identity_only_in_system_not_subagents() {
        assert!(PromptId::System.has_slot(Slot::Identity));
        assert!(!PromptId::Research.has_slot(Slot::Identity));
        assert!(!PromptId::General.has_slot(Slot::Identity));
    }

    #[test]
    fn tone_only_in_system_not_subagents() {
        assert!(PromptId::System.has_slot(Slot::Tone));
        assert!(!PromptId::Research.has_slot(Slot::Tone));
        assert!(!PromptId::General.has_slot(Slot::Tone));
    }

    #[test]
    fn conventions_entry_appends_to_template_defaults() {
        let mut s = ResolvedSlots::default();
        s.insert(
            PromptId::System,
            Slot::Conventions,
            SlotEntry {
                plugin: Arc::from("plugin"),
                content: "- Extra rule".into(),
            },
        );
        let out = assemble(PromptId::System, &s, "");
        assert!(out.contains("Never assume a library is available"));
        assert!(out.contains("- Extra rule"));
    }

    fn write(dir: &std::path::Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn prompt_overrides_init_creates_missing_files() {
        let xdg = tempfile::tempdir().unwrap();
        let overrides = PromptOverrides::load_with_dirs(None, Some(xdg.path()));
        for &(name, _, _) in PROM_DEFS {
            let file = xdg.path().join("prompts").join(format!("{name}.md"));
            assert!(file.exists(), "missing seeded file: {name}");
        }
        assert!(overrides.system.is_some());
        assert!(overrides.plan.is_some());
        assert!(overrides.research.is_some());
        assert!(overrides.general.is_some());
        assert!(overrides.compaction_system.is_some());
        assert!(overrides.compaction_user.is_some());
    }

    #[test]
    fn prompt_overrides_init_does_not_overwrite() {
        let xdg = tempfile::tempdir().unwrap();
        let xdg_prompts = xdg.path().join("prompts");
        fs::create_dir_all(&xdg_prompts).unwrap();
        let plan = xdg_prompts.join("plan.md");
        fs::write(&plan, "USER_CONTENT").unwrap();
        let before = fs::read_to_string(&plan).unwrap();

        let overrides = PromptOverrides::load_with_dirs(None, Some(xdg.path()));
        assert_eq!(fs::read_to_string(&plan).unwrap(), before);
        assert_eq!(overrides.plan.as_deref(), Some("USER_CONTENT"));
    }

    #[test]
    #[cfg(unix)]
    fn prompt_overrides_init_skips_when_xdg_unwritable() {
        let xdg = tempfile::tempdir().unwrap();
        let mut perms = xdg.path().metadata().unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(xdg.path(), perms).unwrap();

        let overrides = PromptOverrides::load_with_dirs(None, Some(xdg.path()));
        assert!(overrides.system.is_none());
        assert!(overrides.plan.is_none());
    }

    #[test]
    fn prompt_overrides_resolution_legacy_wins_over_xdg() {
        let home = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        write(home.path(), ".maki/prompts/plan.md", "LEGACY");
        write(xdg.path(), "prompts/plan.md", "XDG");

        let overrides = PromptOverrides::load_with_dirs(Some(home.path()), Some(xdg.path()));
        assert_eq!(overrides.plan.as_deref(), Some("LEGACY"));
        // init must not rewrite existing files
        assert_eq!(
            fs::read_to_string(xdg.path().join("prompts/plan.md")).unwrap(),
            "XDG"
        );
    }

    #[test]
    fn assemble_with_overrides_plan_mode_marker() {
        let overrides = PromptOverrides::default();
        let with_plan = assemble_with_overrides(
            PromptId::System,
            &ResolvedSlots::default(),
            "",
            &overrides,
            Some(std::path::Path::new("plan.md")),
        );
        assert!(with_plan.contains("Plan Mode"));
        assert!(with_plan.contains("plan.md"));

        let without_plan = assemble_with_overrides(
            PromptId::System,
            &ResolvedSlots::default(),
            "",
            &overrides,
            None,
        );
        assert!(!without_plan.contains("{{plan_mode}}"));
        assert!(!without_plan.contains("Plan Mode"));
    }

    #[test]
    fn assemble_with_overrides_user_plan_replaces_default() {
        let overrides = PromptOverrides {
            plan: Some("MY_PLAN_BODY at {plan_path}".into()),
            ..Default::default()
        };
        let out = assemble_with_overrides(
            PromptId::System,
            &ResolvedSlots::default(),
            "",
            &overrides,
            Some(std::path::Path::new("plan.md")),
        );
        assert!(out.contains("MY_PLAN_BODY at plan.md"));
        assert!(!out.contains("CRITICAL: Plan mode ACTIVE"));
    }

    #[test]
    fn assemble_with_overrides_research_template() {
        let overrides = PromptOverrides {
            research: Some("RESEARCH_TEMPLATE".into()),
            ..Default::default()
        };
        let out = assemble_with_overrides(
            PromptId::Research,
            &ResolvedSlots::default(),
            "",
            &overrides,
            None,
        );
        assert!(out.contains("RESEARCH_TEMPLATE"));
        assert!(!out.contains("You are a research agent"));
    }
}
