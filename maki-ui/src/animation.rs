use std::mem;
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use ratatui::style::{Color, Style};

const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const SPINNER_STRS: [&str; 10] = ["⠋ ", "⠙ ", "⠹ ", "⠸ ", "⠼ ", "⠴ ", "⠦ ", "⠧ ", "⠇ ", "⠏ "];
const SPINNER_FRAME_MS: u128 = 80;

/// The single-glyph "dot" spinner: no per-frame shape animation, just an
/// alternating color (see `dot_spinner_color`).
const DOT_GLYPH: char = '●';
const DOT_STR: &str = "● ";
const DOT_COLOR_PERIOD_MS: u128 = 750;

/// Blank placeholders matching the width of a live glyph, used when spinners
/// are disabled so fixed-width layouts don't shift.
const BLANK_CHAR: char = ' ';
const BLANK_STR: &str = "  ";

/// Which glyph the spinner renders as. `Braille` is the original animated
/// frame cycle; `Dot` is a single glyph whose color alternates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerStyle {
    Braille,
    Dot,
}

/// Global, user-configurable spinner behavior. Mirrors the `theme::current()`
/// / `theme::set()` pattern: deeply-nested render call sites read this
/// directly rather than threading `UserSettings` through every signature.
#[derive(Debug, Clone, Copy)]
pub struct SpinnerConfig {
    pub enabled: bool,
    pub style: SpinnerStyle,
}

impl Default for SpinnerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            style: SpinnerStyle::Braille,
        }
    }
}

static SPINNER_CONFIG: LazyLock<ArcSwap<SpinnerConfig>> =
    LazyLock::new(|| ArcSwap::from_pointee(SpinnerConfig::default()));

pub fn spinner_config() -> Arc<SpinnerConfig> {
    SPINNER_CONFIG.load_full()
}

pub fn set_spinner_config(cfg: SpinnerConfig) {
    SPINNER_CONFIG.store(Arc::new(cfg));
}

pub fn spinner_frame(elapsed_ms: u128) -> char {
    SPINNER_FRAMES[(elapsed_ms / SPINNER_FRAME_MS) as usize % SPINNER_FRAMES.len()]
}

pub fn spinner_str(elapsed_ms: u128) -> &'static str {
    SPINNER_STRS[(elapsed_ms / SPINNER_FRAME_MS) as usize % SPINNER_STRS.len()]
}

/// Config-aware single-char spinner glyph: honors the active enabled/style
/// setting, falling back to a blank space (same width) when disabled.
pub fn active_spinner_frame(elapsed_ms: u128) -> char {
    let cfg = spinner_config();
    if !cfg.enabled {
        return BLANK_CHAR;
    }
    match cfg.style {
        SpinnerStyle::Braille => spinner_frame(elapsed_ms),
        SpinnerStyle::Dot => DOT_GLYPH,
    }
}

/// Config-aware spinner glyph + trailing space, mirroring `spinner_str`.
/// Falls back to a same-width blank when disabled.
pub fn active_spinner_str(elapsed_ms: u128) -> &'static str {
    let cfg = spinner_config();
    if !cfg.enabled {
        return BLANK_STR;
    }
    match cfg.style {
        SpinnerStyle::Braille => spinner_str(elapsed_ms),
        SpinnerStyle::Dot => DOT_STR,
    }
}

/// Color for the dot spinner: alternates between `success` and yellow every
/// `DOT_COLOR_PERIOD_MS`.
pub fn dot_spinner_color(elapsed_ms: u128, success: Color) -> Color {
    if (elapsed_ms / DOT_COLOR_PERIOD_MS).is_multiple_of(2) {
        success
    } else {
        Color::Yellow
    }
}

/// Resolves the `Style` to paint the active spinner glyph with: unchanged
/// for braille, alternating success/yellow fg for dot. `success` supplies
/// the theme's success color (e.g. `theme::current().tool_success`); only
/// its `fg` is used, defaulting to green if unset.
pub fn active_spinner_style(elapsed_ms: u128, base: Style, success: Style) -> Style {
    match spinner_config().style {
        SpinnerStyle::Braille => base,
        SpinnerStyle::Dot => {
            base.fg(dot_spinner_color(elapsed_ms, success.fg.unwrap_or(Color::Green)))
        }
    }
}

/// Spinners need a consistent time reference. Using a static epoch avoids
/// passing Instant through every render call.
pub fn animation_elapsed_ms() -> u128 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis()
}

const DEFAULT_MS_PER_CHAR: u64 = 4;
const MIN_DURATION_MS: u64 = 30;
const MAX_DURATION_MS: u64 = 1000;

pub struct Typewriter {
    buffer: String,
    visible_len: usize,
    visible_byte_offset: usize,
    anim_start_visible: usize,
    anim_target: usize,
    anim_start_at: Instant,
    anim_duration: Duration,
    ms_per_char: u64,
}

impl Default for Typewriter {
    fn default() -> Self {
        Self::with_speed(DEFAULT_MS_PER_CHAR)
    }
}

impl Typewriter {
    pub fn new() -> Self {
        Self::with_speed(DEFAULT_MS_PER_CHAR)
    }

    pub fn with_speed(ms_per_char: u64) -> Self {
        Self {
            buffer: String::new(),
            visible_len: 0,
            visible_byte_offset: 0,
            anim_start_visible: 0,
            anim_target: 0,
            anim_start_at: Instant::now(),
            anim_duration: Duration::ZERO,
            ms_per_char,
        }
    }

    pub fn push(&mut self, text: &str) {
        self.buffer.push_str(text);
        self.tick();
        self.anim_start_visible = self.visible_len;
        self.anim_target = self.buffer.chars().count();
        if self.ms_per_char == 0 {
            self.advance_visible(self.anim_target);
            return;
        }
        let unrevealed = self.anim_target - self.anim_start_visible;
        let ms = (unrevealed as u64 * self.ms_per_char).clamp(MIN_DURATION_MS, MAX_DURATION_MS);
        self.anim_duration = Duration::from_millis(ms);
        self.anim_start_at = Instant::now();
    }

    pub fn tick(&mut self) {
        if self.visible_len >= self.anim_target {
            return;
        }
        let elapsed = self.anim_start_at.elapsed();
        let progress = (elapsed.as_secs_f64() / self.anim_duration.as_secs_f64()).min(1.0);
        let delta = self.anim_target - self.anim_start_visible;
        let new_len = self.anim_start_visible + (delta as f64 * progress).round() as usize;
        self.advance_visible(new_len);
    }

    pub fn visible(&self) -> &str {
        &self.buffer[..self.visible_byte_offset]
    }

    pub fn is_animating(&self) -> bool {
        self.visible_len < self.anim_target
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn visible_len(&self) -> usize {
        self.visible_len
    }

    pub fn buffer_line_count(&self) -> usize {
        if self.buffer.is_empty() {
            0
        } else {
            self.buffer.bytes().filter(|&b| b == b'\n').count() + 1
        }
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.reset_anim();
    }

    pub fn take_all(&mut self) -> String {
        self.reset_anim();
        mem::take(&mut self.buffer)
    }

    #[cfg(test)]
    pub(crate) fn set_buffer(&mut self, text: &str) {
        self.buffer = text.into();
        let len = self.buffer.chars().count();
        self.visible_len = len;
        self.visible_byte_offset = self.buffer.len();
        self.anim_start_visible = len;
        self.anim_target = len;
        self.anim_duration = Duration::ZERO;
    }

    fn reset_anim(&mut self) {
        self.visible_len = 0;
        self.visible_byte_offset = 0;
        self.anim_start_visible = 0;
        self.anim_target = 0;
    }

    fn advance_visible(&mut self, new_len: usize) {
        let skip = new_len - self.visible_len;
        if skip > 0 {
            self.visible_byte_offset = self.buffer[self.visible_byte_offset..]
                .char_indices()
                .nth(skip)
                .map_or(self.buffer.len(), |(i, _)| self.visible_byte_offset + i);
        }
        self.visible_len = new_len;
    }
}

impl PartialEq<&str> for Typewriter {
    fn eq(&self, other: &&str) -> bool {
        self.buffer == *other
    }
}

impl std::fmt::Debug for Typewriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Typewriter")
            .field("buffer", &self.buffer)
            .field("visible_len", &self.visible_len)
            .field("anim_target", &self.anim_target)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `SPINNER_CONFIG` is a process-global; serialize tests that mutate it
    /// so they don't race under cargo's default parallel test execution.
    static SPINNER_CONFIG_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn spinner_wraps_around() {
        let first = spinner_frame(0);
        let wrapped = spinner_frame(SPINNER_FRAME_MS * SPINNER_FRAMES.len() as u128);
        assert_eq!(first, wrapped);
        assert_ne!(first, spinner_frame(SPINNER_FRAME_MS));
    }

    #[test]
    fn dot_color_alternates_every_750ms() {
        let green = Color::Green;
        assert_eq!(dot_spinner_color(0, green), green);
        assert_eq!(dot_spinner_color(749, green), green);
        assert_eq!(dot_spinner_color(750, green), Color::Yellow);
        assert_eq!(dot_spinner_color(1499, green), Color::Yellow);
        assert_eq!(dot_spinner_color(1500, green), green);
    }

    #[test]
    fn disabled_config_suppresses_glyph() {
        let _guard = SPINNER_CONFIG_TEST_LOCK.lock().unwrap();
        let saved = *spinner_config();
        set_spinner_config(SpinnerConfig {
            enabled: false,
            style: SpinnerStyle::Dot,
        });
        assert_eq!(active_spinner_frame(0), BLANK_CHAR);
        assert_eq!(active_spinner_str(0), BLANK_STR);
        // Restore default so other tests relying on spinner_config() (if any
        // are added later) aren't affected by ordering.
        set_spinner_config(saved);
    }

    #[test]
    fn dot_style_overrides_glyph_and_color() {
        let _guard = SPINNER_CONFIG_TEST_LOCK.lock().unwrap();
        let saved = *spinner_config();
        set_spinner_config(SpinnerConfig {
            enabled: true,
            style: SpinnerStyle::Dot,
        });
        assert_eq!(active_spinner_frame(0), DOT_GLYPH);
        assert_eq!(active_spinner_str(0), DOT_STR);
        let base = Style::default();
        let success = Style::default().fg(Color::Green);
        assert_eq!(
            active_spinner_style(0, base, success).fg,
            Some(Color::Green)
        );
        assert_eq!(
            active_spinner_style(750, base, success).fg,
            Some(Color::Yellow)
        );
        set_spinner_config(saved);
    }

    #[test]
    fn push_animates_and_empty_push_is_noop() {
        let mut tw = Typewriter::new();
        tw.push("");
        assert!(!tw.is_animating());
        assert!(tw.is_empty());

        tw.push("hello world, this is a longer string");
        assert_eq!(tw.visible(), "");
        assert!(tw.is_animating());
    }

    #[test]
    fn set_buffer_makes_everything_visible() {
        let mut tw = Typewriter::new();
        tw.set_buffer("héllo 🌍");
        assert_eq!(tw.visible(), "héllo 🌍");
        assert!(!tw.is_animating());
    }

    #[test]
    fn extend_preserves_visible_and_animates_new() {
        let mut tw = Typewriter::new();
        tw.set_buffer("ab");
        tw.push("cdefghijklmnop");
        assert_eq!(tw.visible(), "ab");
        assert!(tw.is_animating());
    }

    #[test]
    fn zero_speed_sequential_pushes_multibyte() {
        let mut tw = Typewriter::with_speed(0);
        tw.push("a");
        tw.push("é");
        tw.push("中");
        tw.push("🦀");
        assert_eq!(tw.visible(), "aé中🦀");
        assert!(!tw.is_animating());
    }

    #[test]
    fn clear_and_take_all_reset_byte_offset() {
        let mut tw = Typewriter::with_speed(0);

        tw.push("🔥🔥🔥");
        assert_eq!(tw.visible(), "🔥🔥🔥");
        tw.clear();
        assert!(tw.is_empty());
        assert_eq!(tw.visible(), "");

        tw.push("日本語");
        assert_eq!(tw.visible(), "日本語");
        let taken = tw.take_all();
        assert_eq!(taken, "日本語");
        assert!(tw.is_empty());
        assert_eq!(tw.visible(), "");

        tw.push("ok");
        assert_eq!(tw.visible(), "ok");
    }

    #[test]
    fn set_buffer_then_push_multibyte() {
        let mut tw = Typewriter::with_speed(0);
        tw.set_buffer("àá");
        tw.push("â🎉ã");
        assert_eq!(tw.visible(), "àáâ🎉ã");
    }

    #[test]
    fn repeated_clear_push_cycles() {
        let mut tw = Typewriter::with_speed(0);
        for _ in 0..3 {
            tw.push("🎵test🎵");
            assert_eq!(tw.visible(), "🎵test🎵");
            tw.clear();
            assert_eq!(tw.visible(), "");
        }
    }

    #[test]
    fn partial_eq_compares_full_buffer() {
        let mut tw = Typewriter::new();
        tw.push("hello world, this is enough text");
        assert_eq!(tw, "hello world, this is enough text");
        assert_eq!(tw.visible(), "");
    }
}
