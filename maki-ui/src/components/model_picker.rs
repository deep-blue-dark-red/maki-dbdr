use std::sync::Arc;

use arc_swap::ArcSwapOption;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use maki_providers::Model;
use maki_providers::ModelTier;
use maki_providers::dynamic;
use maki_providers::model_registry;
use maki_providers::spec::ProviderRegistry;
use maki_storage::aa_scores::AaScores;

use crate::components::Overlay;
use crate::components::list_picker::{ListPicker, PickerAction, PickerItem};
use crate::repaint::{Cadence, Dirty, Watch};
use crate::theme;

const TITLE: &str = " Models ";
const RECENT_SECTION: &str = "Recent";
const FREE_LABEL: &str = "Free";
const SCORE_LABEL: &str = "AA";
const PRICE_SEPARATOR: &str = " · ";
const PRICE_WIDTH: usize = 7;
/// Fits `AA ~100.0`, the widest score a row can quote.
const SCORE_COL_WIDTH: usize = 8;
/// Rank 0 of each fade: best-scoring model, priciest model.
const SCORE_HIGH: Color = Color::Rgb(0x58, 0xa6, 0xff);
const COST_HIGH: Color = Color::Rgb(0xff, 0x51, 0x51);
/// Fade target when the theme's description colour is not an RGB blend.
const RANK_GREY: Color = Color::Rgb(0x6e, 0x6e, 0x6e);

fn footer_line() -> Line<'static> {
    let t = theme::current();
    Line::from(vec![
        Span::styled("  Enter", t.keybind_key),
        Span::styled(" select", t.tool_dim),
        Span::styled("  C", t.keybind_key),
        Span::styled(" cheap default", t.tool_dim),
        Span::styled("  S", t.keybind_key),
        Span::styled(" strong default", t.tool_dim),
        Span::styled("  T", t.keybind_key),
        Span::styled(" thinking (cycle)", t.tool_dim),
    ])
}

fn tier_for_shortcut(key: KeyEvent) -> Option<ModelTier> {
    // Shift+digit arrives as the character it types, with the SHIFT bit
    // already folded into it by key normalization.
    let digit = match key.code {
        KeyCode::Char('!' | '¡') => '1',       // US, ES
        KeyCode::Char('@' | '"' | '™') => '2', // US, UK/DE
        KeyCode::Char('#' | '§' | '£') => '3', // US, DE, UK
        KeyCode::Char('$' | '€' | '¤') => '4', // US, EU, Nordic
        _ => return None,
    };
    match digit {
        '1' => Some(ModelTier::Strong),
        '2' => Some(ModelTier::Medium),
        '3' => Some(ModelTier::Weak),
        '4' => Some(ModelTier::Compaction),
        _ => None,
    }
}

/// `S` and `C` set the strong and cheap defaults the footer advertises;
/// the shifted digits keep their legacy tier shortcuts.
fn tier_shortcut(key: KeyEvent) -> Option<ModelTier> {
    match key.code {
        KeyCode::Char('S') => Some(ModelTier::Strong),
        KeyCode::Char('C') => Some(ModelTier::Weak),
        _ => tier_for_shortcut(key),
    }
}

#[derive(Debug)]
pub enum ModelPickerAction {
    Consumed,
    Select(String),
    AssignTier(String, ModelTier),
    UnassignTier(String, ModelTier),
    CycleThinking,
    Close,
}

/// One quoted number in the detail row: its text, the value behind its rank,
/// and the rank itself (0.0 = brightest end of the fade).
struct Cell {
    value: f64,
    text: String,
    rank: f32,
}

struct ModelEntry {
    spec: String,
    id: String,
    provider_display: String,
    /// Everything a search should reach: display name, slug, and model id.
    search: String,
    suffix: Option<String>,
    score: Option<Cell>,
    price: Option<Cell>,
    /// Width the widest price in the list pads into, so every score sits in
    /// the same column however long the neighbouring prices are.
    price_col: usize,
    override_tiers: Vec<ModelTier>,
    free: bool,
}

impl PickerItem for ModelEntry {
    fn label(&self) -> &str {
        &self.id
    }

    fn search_text(&self) -> &str {
        &self.search
    }

    fn suffix(&self) -> Option<&str> {
        self.suffix.as_deref()
    }

    fn detail_spans(&self) -> Option<Vec<Span<'_>>> {
        if self.score.is_none() && self.price.is_none() {
            return None;
        }
        let t = theme::current();
        let mut spans = Vec::with_capacity(3);
        match &self.score {
            // No price means the price column stays empty, so the score is
            // padded out to where the priced rows begin instead of sliding
            // into it.
            Some(score) => spans.push(Span::styled(
                &score.text,
                rank_style(SCORE_HIGH, score.rank),
            )),
            None => spans.push(Span::raw(" ".repeat(SCORE_COL_WIDTH))),
        }
        match &self.price {
            Some(price) => {
                spans.push(Span::styled(PRICE_SEPARATOR, t.item_desc));
                spans.push(Span::styled(&price.text, rank_style(COST_HIGH, price.rank)));
            }
            None => {
                let gap = PRICE_SEPARATOR.width() + self.price_col;
                spans.push(Span::raw(" ".repeat(gap)));
            }
        }
        Some(spans)
    }

    fn section(&self) -> Option<&str> {
        Some(self.provider_display.as_str())
    }

    fn is_highlighted(&self) -> bool {
        !self.override_tiers.is_empty()
    }
}

pub struct ModelPicker {
    picker: ListPicker<ModelEntry>,
    models: Arc<ArcSwapOption<Vec<String>>>,
    available: Watch<Vec<String>>,
    recents: Vec<String>,
    current_spec: String,
    needs_rebuild: bool,
    /// User-moved entry to restore on refresh: `(was_recent, spec)`.
    anchor: Option<(bool, String)>,
}

impl ModelPicker {
    pub fn new(models: Arc<ArcSwapOption<Vec<String>>>) -> Self {
        Self {
            picker: ListPicker::new().with_footer_builder(footer_line),
            models,
            available: Watch::default(),
            recents: Vec::new(),
            current_spec: String::new(),
            needs_rebuild: false,
            anchor: None,
        }
    }

    pub fn set_recents(&mut self, recents: Vec<String>) {
        self.recents = recents;
        self.needs_rebuild = true;
    }

    pub fn open(&mut self, current_spec: &str) {
        self.current_spec = current_spec.to_owned();
        self.anchor = None;
        self.needs_rebuild = false;
        let _ = self.available.poll(self.models.load_full());
        let entries = self.load_entries();
        self.picker.open(entries, TITLE);
        self.preselect_current_model();
    }

    /// Ranks every quoted number against the whole list: rank 0 is the
    /// brightest end of each fade (best score, priciest model), the last
    /// rank lands on grey.
    fn rank_cells(cells: &mut [&mut Cell]) {
        cells.sort_by(|a, b| b.value.total_cmp(&a.value));
        let last = cells.len().saturating_sub(1);
        for (pos, cell) in cells.iter_mut().enumerate() {
            cell.rank = pos as f32 / last.max(1) as f32;
        }
    }

    /// Providers fetch their model lists in the background and drop them into
    /// a shared slot, which wakes nothing. An open picker has to notice on its
    /// own, so `App::tick` polls this instead of [`Self::view`] reading the
    /// slot mid render.
    pub fn refresh(&mut self) -> Dirty {
        if !self.picker.is_open() {
            return Dirty::NO;
        }
        let arrived = self.available.poll(self.models.load_full());
        if arrived == Dirty::NO && !self.needs_rebuild {
            return Dirty::NO;
        }
        self.needs_rebuild = false;
        let entries = self.load_entries();
        self.picker.replace_items(entries);
        if let Some((was_recent, spec)) = &self.anchor {
            self.picker
                .select_item_by(|e| e.spec == *spec && e.suffix().is_some() == *was_recent);
        } else {
            self.preselect_current_model();
        }
        Dirty::YES
    }

    fn load_entries(&self) -> Vec<ModelEntry> {
        let specs = self.available.get();
        let scores = AaScores::load();
        let mut entries = Vec::new();
        for spec in &self.recents {
            if let Some(mut e) = parse_model_entry(spec, &scores) {
                e.suffix = Some(std::mem::take(&mut e.provider_display));
                e.provider_display = RECENT_SECTION.to_string();
                entries.push(e);
            }
        }
        let mut full: Vec<ModelEntry> = specs
            .map(|s| {
                s.iter()
                    .filter_map(|s| parse_model_entry(s, &scores))
                    .collect()
            })
            .unwrap_or_default();
        full.sort_by(|a, b| {
            a.provider_display
                .cmp(&b.provider_display)
                .then_with(|| b.free.cmp(&a.free))
                .then_with(|| a.id.cmp(&b.id))
        });
        entries.extend(full);
        let mut scored: Vec<&mut Cell> = entries
            .iter_mut()
            .filter_map(|e| e.score.as_mut())
            .collect();
        Self::rank_cells(&mut scored);
        let mut priced: Vec<&mut Cell> = entries
            .iter_mut()
            .filter_map(|e| e.price.as_mut())
            .collect();
        Self::rank_cells(&mut priced);
        let price_col = priced.iter().map(|c| c.text.width()).max().unwrap_or(0);
        for e in &mut entries {
            e.price_col = price_col;
            if let Some(score) = &mut e.score {
                score.text = format!("{:<SCORE_COL_WIDTH$}", score.text);
            }
            if let Some(price) = &mut e.price {
                price.text = format!("{:>price_col$}", price.text);
            }
        }
        entries
    }

    fn preselect_current_model(&mut self) {
        if !self
            .picker
            .select_item_by(|e| e.spec == self.current_spec && e.suffix().is_none())
        {
            self.picker.select_item_by(|e| e.spec == self.current_spec);
        }
    }

    pub fn is_open(&self) -> bool {
        self.picker.is_open()
    }

    pub fn close(&mut self) {
        self.picker.close();
    }

    pub fn contains(&self, pos: Position) -> bool {
        self.picker.contains(pos)
    }

    pub fn scroll(&mut self, delta: i32) {
        self.picker.scroll(delta);
    }

    fn track_anchor<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let before = self.picker.selected_index();
        let result = f(self);
        if let (Some(before), Some(after)) = (before, self.picker.selected_index())
            && before != after
        {
            self.anchor = self
                .picker
                .selected_item()
                .map(|e| (e.suffix().is_some(), e.spec.clone()));
        }
        result
    }

    pub fn handle_paste(&mut self, text: &str) -> bool {
        self.track_anchor(|p| p.picker.handle_paste(text))
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModelPickerAction {
        self.track_anchor(|p| p.handle_key_inner(key))
    }

    fn handle_key_inner(&mut self, key: KeyEvent) -> ModelPickerAction {
        if let Some(tier) = tier_shortcut(key)
            && let Some(entry) = self.picker.selected_item()
        {
            let spec = entry.spec.clone();
            self.needs_rebuild = true;
            if entry.override_tiers.contains(&tier) {
                ModelPickerAction::UnassignTier(spec, tier)
            } else {
                ModelPickerAction::AssignTier(spec, tier)
            }
        } else if key.code == KeyCode::Char('T') {
            ModelPickerAction::CycleThinking
        } else {
            match self.picker.handle_key(key) {
                PickerAction::Consumed => ModelPickerAction::Consumed,
                PickerAction::Select(entry) => ModelPickerAction::Select(entry.spec),
                PickerAction::Close => ModelPickerAction::Close,
                PickerAction::Toggle(..) => ModelPickerAction::Consumed,
            }
        }
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        self.picker.view(frame, area)
    }
}

impl Overlay for ModelPicker {
    fn is_open(&self) -> bool {
        self.is_open()
    }

    fn close(&mut self) {
        self.close()
    }

    fn cadence(&self) -> Cadence {
        self.picker.cadence()
    }
}

fn format_pricing(model: &Model) -> Option<String> {
    let pricing = &model.pricing;
    (model.subsidised_by.is_none() && !pricing.is_zero()).then(|| {
        format!(
            "{:>PRICE_WIDTH$}/{:<PRICE_WIDTH$}",
            format!("${:.2}", pricing.input),
            format!("${:.2}", pricing.output),
        )
    })
}

/// The grey end of both fades: the theme's own description colour when it can
/// blend, a fixed grey otherwise.
fn rank_grey(t: &crate::theme::Theme) -> Color {
    match t.item_desc.fg {
        Some(c @ Color::Rgb(..)) => c,
        _ => RANK_GREY,
    }
}

fn blend(from: Color, to: Color, t: f32) -> Color {
    match (from, to) {
        (Color::Rgb(fr, fg, fb), Color::Rgb(tr, tg, tb)) => Color::Rgb(
            theme::lerp_u8(fr, tr, t),
            theme::lerp_u8(fg, tg, t),
            theme::lerp_u8(fb, tb, t),
        ),
        _ => from,
    }
}

fn rank_style(top: Color, rank: f32) -> Style {
    Style::default().fg(blend(top, rank_grey(&theme::current()), rank))
}

fn parse_model_entry(spec: &str, scores: &AaScores) -> Option<ModelEntry> {
    let (provider_str, model_id) = spec.split_once('/')?;

    // `opencode-go` has a spec row but was never a `ProviderKind`, so the
    // catalog named it and still should. That is what `is_native` filters for.
    let provider_display =
        if let Some(spec) = ProviderRegistry::get(provider_str).filter(|s| s.is_native()) {
            spec.display_name.to_string()
        } else if let Some(name) = dynamic::display_name(provider_str) {
            name.to_string()
        } else if let Some(info) = maki_providers::catalog_provider_if_available(provider_str) {
            info.display_name.clone()
        } else if let Some(builtin) = maki_config::providers::builtin_provider(provider_str) {
            builtin.display_name.to_string()
        } else {
            let config = maki_config::providers::ProvidersConfig::load();
            config.get(provider_str)?;
            maki_config::providers::resolve_display_name(provider_str, config.get(provider_str))
        };

    let override_tiers = model_registry::override_tiers(spec);
    let (free, price) = match Model::from_spec(spec) {
        Ok(m) => {
            let free = m.is_free();
            let price = match format_pricing(&m) {
                Some(text) => Some(Cell {
                    value: m.pricing.input + m.pricing.output,
                    text,
                    rank: 0.0,
                }),
                None if free => Some(Cell {
                    value: 0.0,
                    text: FREE_LABEL.to_string(),
                    rank: 0.0,
                }),
                None => None,
            };
            (free, price)
        }
        Err(_) => (false, None),
    };
    // The row quotes the intelligence index first, then what it costs to run:
    // a tier word told the reader nothing the tier shortcuts did not already.
    let score = scores.score(model_id).map(|(value, estimated)| Cell {
        value,
        text: if estimated {
            format!("{SCORE_LABEL} ~{value:.1}")
        } else {
            format!("{SCORE_LABEL} {value:.1}")
        },
        rank: 0.0,
    });
    let id = model_id.to_string();
    Some(ModelEntry {
        spec: spec.to_string(),
        id,
        search: format!("{provider_display} {spec}"),
        provider_display,
        suffix: None,
        score,
        price,
        price_col: 0,
        override_tiers,
        free,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::key;
    use crate::components::keybindings::key as kb;
    use crossterm::event::{KeyCode, KeyEvent};
    use maki_providers::ModelInfo;
    use maki_providers::ModelPricing;
    use test_case::test_case;

    const SAME_SIZED_LIST: &str = "a republished list of the same length is still a new list";
    const SWAPPED_SPEC: &str = "zai/glm-5";

    /// A provider that republishes the same number of specs has still changed
    /// the list. Comparing lengths calls that no change, and the picker goes on
    /// offering models that are gone.
    #[test]
    fn a_same_sized_model_list_owes_a_frame() {
        let models = Arc::new(ArcSwapOption::empty());
        models.store(Some(Arc::new(vec![
            "anthropic/claude-sonnet-4-20250514".into(),
        ])));
        let mut p = ModelPicker::new(Arc::clone(&models));
        p.open("");
        assert_eq!(p.refresh(), Dirty::NO);

        models.store(Some(Arc::new(vec![SWAPPED_SPEC.into()])));
        assert_eq!(p.refresh(), Dirty::YES, "{SAME_SIZED_LIST}");
        assert_eq!(
            p.picker.selected_item().map(|e| e.spec.as_str()),
            Some(SWAPPED_SPEC),
            "{SAME_SIZED_LIST}"
        );
    }

    fn test_models() -> Arc<ArcSwapOption<Vec<String>>> {
        let models = Arc::new(ArcSwapOption::empty());
        models.store(Some(Arc::new(vec![
            "anthropic/claude-sonnet-4-20250514".into(),
            "anthropic/claude-opus-4-6-20260101".into(),
            "zai/glm-5".into(),
        ])));
        models
    }

    #[test_case(key(KeyCode::Esc)          ; "esc_closes")]
    #[test_case(kb::QUIT.to_key_event()    ; "ctrl_c_closes")]
    fn close_keys(cancel_key: KeyEvent) {
        let mut p = ModelPicker::new(test_models());
        p.open("");
        let action = p.handle_key(cancel_key);
        assert!(matches!(action, ModelPickerAction::Close));
        assert!(!p.is_open());
    }

    #[test]
    fn refresh_updates_items_and_preserves_search() {
        let models = Arc::new(ArcSwapOption::empty());
        models.store(Some(Arc::new(vec![
            "anthropic/claude-sonnet-4-20250514".into(),
        ])));
        let mut p = ModelPicker::new(models.clone());
        p.open("");

        p.handle_key(key(KeyCode::Char('o')));
        p.handle_key(key(KeyCode::Char('p')));

        models.store(Some(Arc::new(vec![
            "anthropic/claude-sonnet-4-20250514".into(),
            "anthropic/claude-opus-4-6-20260101".into(),
        ])));
        let _ = p.refresh();

        let action = p.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, ModelPickerAction::Select(ref s) if s.contains("opus")),
            "after refresh, 'op' filter should match opus"
        );
    }

    #[test]
    fn open_preselects_current_model() {
        let mut p = ModelPicker::new(test_models());
        p.open("anthropic/claude-opus-4-6-20260101");
        let action = p.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, ModelPickerAction::Select(ref s) if s == "anthropic/claude-opus-4-6-20260101")
        );
    }

    /// The label is only the model id, so a provider named in the query must
    /// still reach its rows: display name and slug are search text too.
    #[test_case("zai", SWAPPED_SPEC          ; "slug")]
    #[test_case("anthropic", "anthropic/claude-opus-4-6-20260101" ; "display_name_is_also_the_slug")]
    fn provider_name_query_finds_models(query: &str, expected: &str) {
        let mut p = ModelPicker::new(test_models());
        p.open("");
        for ch in query.chars() {
            p.handle_key(key(KeyCode::Char(ch)));
        }
        let action = p.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, ModelPickerAction::Select(ref s) if s == expected),
            "query '{query}' should select {expected}, got {action:?}"
        );
    }

    #[test]
    fn parse_model_entry_valid() {
        let entry =
            parse_model_entry("anthropic/claude-sonnet-4-20250514", &AaScores::default()).unwrap();
        assert_eq!(entry.id, "claude-sonnet-4-20250514");
        assert_eq!(entry.provider_display, "Anthropic");
        assert!(entry.price.is_some());
    }

    #[test]
    fn parse_model_entry_paid_model_not_marked_free() {
        let entry =
            parse_model_entry("anthropic/claude-sonnet-4-20250514", &AaScores::default()).unwrap();
        assert!(!entry.free, "paid anthropic model must not be marked free");
    }

    #[test]
    fn parse_model_entry_no_slash() {
        assert!(parse_model_entry("no-slash", &AaScores::default()).is_none());
    }

    #[test_case(key(KeyCode::Char('!')),           ModelTier::Strong     ; "legacy_bang_strong")]
    #[test_case(key(KeyCode::Char('$')),           ModelTier::Compaction ; "legacy_dollar_compaction")]
    #[test_case(key(KeyCode::Char('€')),           ModelTier::Compaction ; "legacy_euro_compaction")]
    fn tier_shortcut_assigns_and_keeps_picker_open(k: KeyEvent, want: ModelTier) {
        let mut p = ModelPicker::new(test_models());
        p.open("anthropic/claude-sonnet-4-20250514");
        let action = p.handle_key(k);
        assert!(
            matches!(&action, ModelPickerAction::AssignTier(s, t)
                if s == "anthropic/claude-sonnet-4-20250514" && *t == want),
            "expected AssignTier(claude-sonnet, {want:?}), got something else",
        );
        assert!(p.is_open());
    }

    #[test]
    fn refresh_preserves_selection_for_current_model() {
        let models = Arc::new(ArcSwapOption::empty());
        let mut p = ModelPicker::new(models.clone());
        p.open("anthropic/claude-opus-4-6-20260101");

        models.store(Some(Arc::new(vec![
            "anthropic/claude-sonnet-4-20250514".into(),
            "anthropic/claude-opus-4-6-20260101".into(),
            "zai/glm-5".into(),
        ])));
        let _ = p.refresh();

        let action = p.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, ModelPickerAction::Select(ref s) if s == "anthropic/claude-opus-4-6-20260101"),
            "after async model arrival, current model should still be selected"
        );
    }

    #[test]
    fn recents_include_current_model_preselected() {
        let models = test_models();
        let mut p = ModelPicker::new(models);
        p.set_recents(vec![
            "zai/glm-5".into(),
            "anthropic/claude-sonnet-4-20250514".into(),
        ]);
        p.open("anthropic/claude-opus-4-6-20260101");

        p.picker.select(0);
        let action = p.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, ModelPickerAction::Select(ref s) if s == "zai/glm-5"),
            "first entry should be the most recent model",
        );

        p.set_recents(vec![
            "zai/glm-5".into(),
            "anthropic/claude-sonnet-4-20250514".into(),
        ]);
        p.open("zai/glm-5");
        let action = p.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, ModelPickerAction::Select(ref s) if s == "zai/glm-5"),
            "current model should be preselected in its provider section",
        );
    }

    #[test]
    fn reopen_preselects_current_model_in_provider_section() {
        let models = test_models();
        let mut p = ModelPicker::new(models);
        p.set_recents(vec![
            "zai/glm-5".into(),
            "anthropic/claude-sonnet-4-20250514".into(),
        ]);
        p.open("anthropic/claude-sonnet-4-20250514");
        p.handle_key(key(KeyCode::Down));
        let action = p.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, ModelPickerAction::Select(ref s) if s == "zai/glm-5"),
            "selecting the provider entry should return its spec",
        );

        p.open("zai/glm-5");

        let entry = p.picker.selected_item().expect("selection on reopen");
        assert_eq!(entry.spec, "zai/glm-5");
        assert_eq!(
            entry.section(),
            Some("Z.AI"),
            "selection should land on the provider entry, not the Recent copy",
        );
    }

    #[test]
    fn refresh_keeps_selection_on_provider_entry() {
        let models = test_models();
        let mut p = ModelPicker::new(models);
        p.set_recents(vec![
            "zai/glm-5".into(),
            "anthropic/claude-sonnet-4-20250514".into(),
        ]);
        p.open("anthropic/claude-sonnet-4-20250514");
        p.handle_key(key(KeyCode::Down));
        p.handle_key(key(KeyCode::Char('!')));

        let _ = p.refresh();

        let entry = p.picker.selected_item().expect("selection after refresh");
        assert_eq!(entry.spec, "zai/glm-5");
        assert_eq!(
            entry.section(),
            Some("Z.AI"),
            "selection should stay on the provider entry, not jump to Recent",
        );
    }

    #[test]
    fn refresh_after_collapse_anchors_to_provider_entry() {
        let models = test_models();
        let mut p = ModelPicker::new(models.clone());
        p.set_recents(vec![
            "zai/glm-5".into(),
            "anthropic/claude-sonnet-4-20250514".into(),
        ]);
        p.open("anthropic/claude-sonnet-4-20250514");

        models.store(None);
        let _ = p.refresh();
        let entry = p.picker.selected_item().expect("selection during collapse");
        assert_eq!(entry.spec, "anthropic/claude-sonnet-4-20250514");
        assert_eq!(entry.section(), Some("Recent"));

        models.store(Some(Arc::new(vec![
            "anthropic/claude-sonnet-4-20250514".into(),
            "anthropic/claude-opus-4-6-20260101".into(),
            "zai/glm-5".into(),
        ])));
        let _ = p.refresh();

        let entry = p.picker.selected_item().expect("selection after arrival");
        assert_eq!(entry.spec, "anthropic/claude-sonnet-4-20250514");
        assert_eq!(
            entry.section(),
            Some("Anthropic"),
            "cursor should migrate to the provider entry once it arrives",
        );
    }

    #[test]
    fn refresh_preserves_navigation_to_recent_entry() {
        let models = test_models();
        let mut p = ModelPicker::new(models.clone());
        p.set_recents(vec![
            "zai/glm-5".into(),
            "anthropic/claude-sonnet-4-20250514".into(),
        ]);
        p.open("anthropic/claude-sonnet-4-20250514");
        models.store(None);
        let _ = p.refresh();
        p.handle_key(key(KeyCode::Down));

        models.store(Some(Arc::new(vec![
            "anthropic/claude-sonnet-4-20250514".into(),
            "anthropic/claude-opus-4-6-20260101".into(),
            "zai/glm-5".into(),
        ])));
        let _ = p.refresh();

        let entry = p.picker.selected_item().expect("selection after arrival");
        assert_eq!(entry.spec, "zai/glm-5");
        assert_eq!(
            entry.section(),
            Some("Recent"),
            "user navigation to a Recent entry should survive refresh",
        );
    }

    #[test]
    fn refresh_preserves_selection_with_active_search() {
        let models = test_models();
        let mut p = ModelPicker::new(models.clone());
        p.set_recents(vec![
            "zai/glm-5".into(),
            "anthropic/claude-sonnet-4-20250514".into(),
        ]);
        p.open("anthropic/claude-sonnet-4-20250514");
        p.handle_key(key(KeyCode::Char('g')));
        p.handle_key(key(KeyCode::Char('l')));
        p.handle_key(key(KeyCode::Char('m')));

        models.store(None);
        let _ = p.refresh();
        models.store(Some(Arc::new(vec![
            "anthropic/claude-sonnet-4-20250514".into(),
            "anthropic/claude-opus-4-6-20260101".into(),
            "zai/glm-5".into(),
        ])));
        let _ = p.refresh();

        let entry = p.picker.selected_item().expect("selection after refresh");
        assert_eq!(entry.spec, "zai/glm-5");
        assert_eq!(entry.section(), Some("Z.AI"));
    }

    fn discovered(id: &str, pricing: ModelPricing) -> ModelInfo {
        ModelInfo {
            pricing: Some(pricing),
            ..ModelInfo::id_only(id.into())
        }
    }

    const OX_SPEC: &str = "openrouter/stealth/ox-alpha";
    const PAID_ID: &str = "vendor/paid-model";
    const PAID_PRICING: ModelPricing = ModelPricing::per_million(3.0, 15.0, 0.0, 0.0);
    const PAID_PRICE_LABEL: &str = "  $3.00/$15.00 ";

    fn register_openrouter_models() {
        model_registry::set_known_models(
            "openrouter",
            vec![
                discovered("stealth/ox-alpha", ModelPricing::ZERO),
                discovered(PAID_ID, PAID_PRICING),
            ],
        );
    }

    #[test]
    fn zero_priced_discovery_marks_entry_free() {
        register_openrouter_models();
        let entry = parse_model_entry(OX_SPEC, &AaScores::default()).unwrap();
        assert!(
            entry.free && entry.price.as_ref().map(|p| p.text.as_str()) == Some(FREE_LABEL),
            "zero-priced discovery must mark the entry free"
        );
    }

    #[test]
    fn paid_discovery_not_marked_free() {
        register_openrouter_models();
        let entry =
            parse_model_entry(&format!("openrouter/{PAID_ID}"), &AaScores::default()).unwrap();
        assert!(!entry.free, "paid discovery must not mark the entry free");
        assert_eq!(
            entry.price.as_ref().map(|p| p.text.as_str()),
            Some(PAID_PRICE_LABEL),
            "paid discovery must show its price"
        );
    }

    #[test]
    fn detail_quotes_the_cached_score_instead_of_a_tier() {
        register_openrouter_models();
        let scores = AaScores::parse(
            r#"{"scores":{"paid-model":44.8,"stealth/ox-alpha":27.9},"estimated":["stealth/ox-alpha"]}"#,
        )
        .unwrap();

        let paid = parse_model_entry(&format!("openrouter/{PAID_ID}"), &scores).unwrap();
        assert_eq!(
            paid.score.as_ref().map(|s| s.text.as_str()),
            Some("AA 44.8"),
            "a measured score leads the row, tier words are gone"
        );

        let free = parse_model_entry(OX_SPEC, &scores).unwrap();
        assert_eq!(
            free.score.as_ref().map(|s| s.text.as_str()),
            Some("AA ~27.9")
        );
        assert_eq!(
            free.price.as_ref().map(|p| p.text.as_str()),
            Some(FREE_LABEL)
        );
    }

    fn plain_entry(score: Option<Cell>, price: Option<Cell>, price_col: usize) -> ModelEntry {
        ModelEntry {
            spec: String::new(),
            id: String::new(),
            provider_display: String::new(),
            search: String::new(),
            suffix: None,
            score,
            price,
            price_col,
            override_tiers: Vec::new(),
            free: false,
        }
    }

    fn spans_width(e: &ModelEntry) -> usize {
        e.detail_spans()
            .map_or(0, |spans| spans.iter().map(|s| s.content.width()).sum())
    }

    /// Every row with any detail quotes into the same total width, so the
    /// render's right alignment lands scores and prices in fixed columns.
    #[test]
    fn detail_columns_align_across_priced_and_scoreless_rows() {
        const PRICE_COL: usize = 12;
        let cell = |text: &str| Cell {
            value: 1.0,
            text: text.to_string(),
            rank: 0.0,
        };
        let score_text = format!("{:<SCORE_COL_WIDTH$}", format!("{SCORE_LABEL} 40.0"));
        let price_text = format!("{:>PRICE_COL$}", "$3.00/$15.00");
        let both = plain_entry(Some(cell(&score_text)), Some(cell(&price_text)), PRICE_COL);
        let score_only = plain_entry(Some(cell(&score_text)), None, PRICE_COL);
        let price_only = plain_entry(None, Some(cell(&price_text)), PRICE_COL);

        let width = spans_width(&both);
        assert_eq!(spans_width(&score_only), width);
        assert_eq!(spans_width(&price_only), width);

        // A scoreless row leaves the price column blank instead of sliding
        // its score into it.
        let score_only_spans = score_only.detail_spans().unwrap();
        let tail: String = score_only_spans
            .iter()
            .skip(1)
            .map(|s| s.content.to_string())
            .collect();
        assert!(tail.chars().all(char::is_whitespace));

        assert!(plain_entry(None, None, PRICE_COL).detail_spans().is_none());
    }

    /// Rank 0 is the brightest end of a fade, the last rank lands on grey.
    #[test]
    fn ranks_fade_from_bright_to_grey() {
        let mut best = Cell {
            value: 50.0,
            text: String::new(),
            rank: 1.0,
        };
        let mut worst = Cell {
            value: 10.0,
            text: String::new(),
            rank: 0.0,
        };
        ModelPicker::rank_cells(&mut [&mut best, &mut worst]);
        assert_eq!(best.rank, 0.0, "highest value ranks first");
        assert_eq!(worst.rank, 1.0);
        // Costs rank the same way, priciest first.
        let mut cheap = Cell {
            value: 0.0,
            text: String::new(),
            rank: 1.0,
        };
        ModelPicker::rank_cells(&mut [&mut cheap, &mut best, &mut worst]);
        assert_eq!(best.rank, 0.0, "value 50 is the priciest");
        assert_eq!(worst.rank, 0.5);
        assert_eq!(cheap.rank, 1.0, "free fades to grey");
    }

    #[test]
    fn load_entries_pads_prices_into_a_shared_column() {
        register_openrouter_models();
        let models = Arc::new(ArcSwapOption::empty());
        models.store(Some(Arc::new(vec![
            format!("openrouter/{PAID_ID}"),
            OX_SPEC.into(),
        ])));
        let mut p = ModelPicker::new(models);
        p.open("");
        let entries = p.load_entries();
        let paid = entries.iter().find(|e| e.id == PAID_ID).unwrap();
        let free = entries.iter().find(|e| e.id == "stealth/ox-alpha").unwrap();
        assert_eq!(paid.price_col, PAID_PRICE_LABEL.width());
        assert_eq!(paid.price.as_ref().unwrap().text, PAID_PRICE_LABEL);
        assert_eq!(
            paid.price.as_ref().unwrap().rank,
            0.0,
            "priciest is reddest"
        );
        assert_eq!(free.price.as_ref().unwrap().rank, 1.0, "free fades to grey");
    }

    #[test_case(key(KeyCode::Char('C')), ModelTier::Weak     ; "c_sets_cheap_default")]
    #[test_case(key(KeyCode::Char('S')), ModelTier::Strong   ; "s_sets_strong_default")]
    fn default_shortcut_assigns_and_keeps_picker_open(k: KeyEvent, want: ModelTier) {
        let mut p = ModelPicker::new(test_models());
        p.open("anthropic/claude-sonnet-4-20250514");
        let action = p.handle_key(k);
        assert!(matches!(&action, ModelPickerAction::AssignTier(s, t)
                if s == "anthropic/claude-sonnet-4-20250514" && *t == want),);
        assert!(p.is_open());
    }

    #[test]
    fn t_cycles_thinking_without_closing() {
        let mut p = ModelPicker::new(test_models());
        p.open("anthropic/claude-sonnet-4-20250514");
        assert!(matches!(
            p.handle_key(key(KeyCode::Char('T'))),
            ModelPickerAction::CycleThinking
        ));
        assert!(p.is_open());
    }

    #[test]
    fn lowercase_shortcut_letters_still_search() {
        let mut p = ModelPicker::new(test_models());
        p.open("");
        p.handle_key(key(KeyCode::Char('c')));
        let action = p.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, ModelPickerAction::Select(ref s) if s.contains("claude")),
            "lowercase c filters instead of triggering the cheap default"
        );
    }

    #[test]
    fn free_models_sort_before_paid_within_a_provider() {
        register_openrouter_models();
        let models = Arc::new(ArcSwapOption::empty());
        models.store(Some(Arc::new(vec![
            format!("openrouter/{PAID_ID}"),
            OX_SPEC.into(),
        ])));
        let mut p = ModelPicker::new(models);
        p.open("");
        let entries = p.load_entries();
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["stealth/ox-alpha", PAID_ID]);
    }
}
