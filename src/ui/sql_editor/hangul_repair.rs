//! Repair for the macOS "first Hangul key after an input-source switch
//! bypasses the IME" bug.
//!
//! Mechanism (confirmed with `verify_ime_minimal` traces): after every switch
//! to a Korean input source, the first Hangul-composing keystroke is committed
//! directly as a lone jamo (`Fl::compose_state` stays 0, nothing is marked)
//! instead of opening an IME composition session. The IME starts composing
//! only from the second keystroke, so typing 장영환 yields "ㅈㅏㅇ영환".
//! The same deterministic bug is reported against other non-native apps
//! (ghostty #12541); there is no upstream FLTK fix, and re-activating the
//! NSTextInputContext does not help.
//!
//! Detection is exact: during healthy composition every Hangul KeyDown
//! arrives with `compose_state > 0` and the syllable marked; only the broken
//! first key inserts a lone jamo with `compose_state == 0`. Once armed, the
//! committed text between the stranded jamo and the live marked region is
//! recomposed (jamo runs merged into syllables) after every KeyDown, which
//! converges to the native-equivalent display within one keystroke.

use crate::utils::arithmetic::{safe_div, safe_rem};

/// Choseong (initial consonant) index for a compatibility jamo, if any.
fn compat_choseong(ch: char) -> Option<u8> {
    Some(match ch {
        'ㄱ' => 0,
        'ㄲ' => 1,
        'ㄴ' => 2,
        'ㄷ' => 3,
        'ㄸ' => 4,
        'ㄹ' => 5,
        'ㅁ' => 6,
        'ㅂ' => 7,
        'ㅃ' => 8,
        'ㅅ' => 9,
        'ㅆ' => 10,
        'ㅇ' => 11,
        'ㅈ' => 12,
        'ㅉ' => 13,
        'ㅊ' => 14,
        'ㅋ' => 15,
        'ㅌ' => 16,
        'ㅍ' => 17,
        'ㅎ' => 18,
        _ => return None,
    })
}

/// Jongseong (final consonant) index 1..=27 for a compatibility jamo, if any.
fn compat_jongseong(ch: char) -> Option<u8> {
    Some(match ch {
        'ㄱ' => 1,
        'ㄲ' => 2,
        'ㄳ' => 3,
        'ㄴ' => 4,
        'ㄵ' => 5,
        'ㄶ' => 6,
        'ㄷ' => 7,
        'ㄹ' => 8,
        'ㄺ' => 9,
        'ㄻ' => 10,
        'ㄼ' => 11,
        'ㄽ' => 12,
        'ㄾ' => 13,
        'ㄿ' => 14,
        'ㅀ' => 15,
        'ㅁ' => 16,
        'ㅂ' => 17,
        'ㅄ' => 18,
        'ㅅ' => 19,
        'ㅆ' => 20,
        'ㅇ' => 21,
        'ㅈ' => 22,
        'ㅊ' => 23,
        'ㅋ' => 24,
        'ㅌ' => 25,
        'ㅍ' => 26,
        'ㅎ' => 27,
        _ => return None,
    })
}

const CHOSEONG_COMPAT: [char; 19] = [
    'ㄱ', 'ㄲ', 'ㄴ', 'ㄷ', 'ㄸ', 'ㄹ', 'ㅁ', 'ㅂ', 'ㅃ', 'ㅅ', 'ㅆ', 'ㅇ', 'ㅈ', 'ㅉ', 'ㅊ', 'ㅋ',
    'ㅌ', 'ㅍ', 'ㅎ',
];

/// (existing jungseong, typed jungseong) -> compound jungseong.
const COMPOUND_JUNGSEONG: [(u8, u8, u8); 7] = [
    (8, 0, 9),    // ㅗ + ㅏ = ㅘ
    (8, 1, 10),   // ㅗ + ㅐ = ㅙ
    (8, 20, 11),  // ㅗ + ㅣ = ㅚ
    (13, 4, 14),  // ㅜ + ㅓ = ㅝ
    (13, 5, 15),  // ㅜ + ㅔ = ㅞ
    (13, 20, 16), // ㅜ + ㅣ = ㅟ
    (18, 20, 19), // ㅡ + ㅣ = ㅢ
];

/// (existing jongseong, typed jongseong) -> double jongseong.
const DOUBLE_JONGSEONG: [(u8, u8, u8); 11] = [
    (1, 19, 3),   // ㄱ + ㅅ = ㄳ
    (4, 22, 5),   // ㄴ + ㅈ = ㄵ
    (4, 27, 6),   // ㄴ + ㅎ = ㄶ
    (8, 1, 9),    // ㄹ + ㄱ = ㄺ
    (8, 16, 10),  // ㄹ + ㅁ = ㄻ
    (8, 17, 11),  // ㄹ + ㅂ = ㄼ
    (8, 19, 12),  // ㄹ + ㅅ = ㄽ
    (8, 25, 13),  // ㄹ + ㅌ = ㄾ
    (8, 26, 14),  // ㄹ + ㅍ = ㄿ
    (8, 27, 15),  // ㄹ + ㅎ = ㅀ
    (17, 19, 18), // ㅂ + ㅅ = ㅄ
];

/// jongseong index -> choseong index of the same consonant (single jamo only).
fn jongseong_to_choseong(t: u8) -> Option<u8> {
    Some(match t {
        1 => 0,
        2 => 1,
        4 => 2,
        7 => 3,
        8 => 5,
        16 => 6,
        17 => 7,
        19 => 9,
        20 => 10,
        21 => 11,
        22 => 12,
        23 => 14,
        24 => 15,
        25 => 16,
        26 => 17,
        27 => 18,
        _ => return None,
    })
}

/// double jongseong -> (remaining jongseong, choseong of the split-off part).
fn split_double_jongseong(t: u8) -> Option<(u8, u8)> {
    Some(match t {
        3 => (1, 9),   // ㄳ -> ㄱ + ㅅ
        5 => (4, 12),  // ㄵ -> ㄴ + ㅈ
        6 => (4, 18),  // ㄶ -> ㄴ + ㅎ
        9 => (8, 0),   // ㄺ -> ㄹ + ㄱ
        10 => (8, 6),  // ㄻ -> ㄹ + ㅁ
        11 => (8, 7),  // ㄼ -> ㄹ + ㅂ
        12 => (8, 9),  // ㄽ -> ㄹ + ㅅ
        13 => (8, 16), // ㄾ -> ㄹ + ㅌ
        14 => (8, 17), // ㄿ -> ㄹ + ㅍ
        15 => (8, 18), // ㅀ -> ㄹ + ㅎ
        18 => (17, 9), // ㅄ -> ㅂ + ㅅ
        _ => return None,
    })
}

#[derive(Clone, Copy)]
enum Unit {
    /// A consonant jamo: possible choseong index, possible jongseong index,
    /// and the original char for raw emission.
    Consonant(Option<u8>, Option<u8>, char),
    /// A vowel jamo: jungseong index and the original char.
    Vowel(u8, char),
    /// A precomposed syllable, decomposed to (choseong, jungseong, jongseong).
    Syllable(u8, u8, u8),
}

fn classify(ch: char) -> Option<Unit> {
    let code = ch as u32;
    match code {
        // Precomposed Hangul syllables.
        0xAC00..=0xD7A3 => {
            let offset = code - 0xAC00;
            Some(Unit::Syllable(
                safe_div(offset, 588) as u8,
                safe_div(safe_rem(offset, 588), 28) as u8,
                safe_rem(offset, 28) as u8,
            ))
        }
        // Compatibility jamo consonants.
        0x3131..=0x314E => Some(Unit::Consonant(
            compat_choseong(ch),
            compat_jongseong(ch),
            ch,
        )),
        // Compatibility jamo vowels (contiguous, same order as jungseong).
        0x314F..=0x3163 => Some(Unit::Vowel((code - 0x314F) as u8, ch)),
        // Conjoining jamo (NFD forms).
        0x1100..=0x1112 => {
            let l = (code - 0x1100) as u8;
            Some(Unit::Consonant(Some(l), None, CHOSEONG_COMPAT[l as usize]))
        }
        0x1161..=0x1175 => {
            let v = (code - 0x1161) as u8;
            Some(Unit::Vowel(
                v,
                char::from_u32(0x314F + v as u32).unwrap_or(ch),
            ))
        }
        0x11A8..=0x11C2 => Some(Unit::Consonant(None, Some((code - 0x11A8 + 1) as u8), ch)),
        _ => None,
    }
}

fn unit_is_vowel(unit: Option<&Unit>) -> bool {
    matches!(unit, Some(Unit::Vowel(..)))
}

#[derive(Default)]
struct Automaton {
    output: String,
    choseong: Option<u8>,
    jungseong: Option<u8>,
    jongseong: Option<u8>,
}

impl Automaton {
    fn flush(&mut self) {
        match (
            self.choseong.take(),
            self.jungseong.take(),
            self.jongseong.take(),
        ) {
            (Some(l), Some(v), t) => {
                let code = 0xAC00 + (l as u32 * 21 + v as u32) * 28 + t.unwrap_or(0) as u32;
                if let Some(ch) = char::from_u32(code) {
                    self.output.push(ch);
                }
            }
            (Some(l), None, _) => self.output.push(CHOSEONG_COMPAT[l as usize]),
            _ => {}
        }
    }

    fn consonant(&mut self, l: Option<u8>, t: Option<u8>, raw: char, next_is_vowel: bool) {
        match (self.choseong, self.jungseong, self.jongseong) {
            // No syllable in progress: start one if the consonant can lead.
            (None, ..) => match l {
                Some(l) => self.choseong = Some(l),
                None => {
                    self.flush();
                    self.output.push(raw);
                }
            },
            // Bare choseong followed by another consonant never combines.
            (Some(_), None, _) => {
                self.flush();
                self.consonant(l, t, raw, next_is_vowel);
            }
            // Open LV syllable: attach as jongseong unless the next unit is a
            // vowel (then this consonant is the next syllable's choseong).
            (Some(_), Some(_), None) => match t {
                Some(t) if !next_is_vowel => self.jongseong = Some(t),
                _ => {
                    self.flush();
                    self.consonant(l, t, raw, next_is_vowel);
                }
            },
            // Closed LVT syllable: try to extend into a double jongseong.
            (Some(_), Some(_), Some(existing)) => {
                let doubled = t.and_then(|t| {
                    DOUBLE_JONGSEONG
                        .iter()
                        .find(|(a, b, _)| *a == existing && *b == t)
                        .map(|(_, _, d)| *d)
                });
                match doubled {
                    Some(d) if !next_is_vowel => self.jongseong = Some(d),
                    _ => {
                        self.flush();
                        self.consonant(l, t, raw, next_is_vowel);
                    }
                }
            }
        }
    }

    fn vowel(&mut self, v: u8, raw: char) {
        match (self.choseong, self.jungseong, self.jongseong) {
            (Some(_), None, _) => self.jungseong = Some(v),
            // Try compound jungseong (ㅗ+ㅏ=ㅘ, ...).
            (Some(_), Some(existing), None) => {
                let compound = COMPOUND_JUNGSEONG
                    .iter()
                    .find(|(a, b, _)| *a == existing && *b == v)
                    .map(|(_, _, c)| *c);
                match compound {
                    Some(c) => self.jungseong = Some(c),
                    None => {
                        self.flush();
                        self.output.push(raw);
                    }
                }
            }
            // Dokkaebibul: the jongseong moves to the next syllable as its
            // choseong when a vowel follows.
            (Some(_), Some(_), Some(t)) => {
                let moved = jongseong_to_choseong(t).map(|l| (None, l)).or_else(|| {
                    split_double_jongseong(t).map(|(remaining, l)| (Some(remaining), l))
                });
                match moved {
                    Some((remaining, l)) => {
                        self.jongseong = remaining;
                        self.flush();
                        self.choseong = Some(l);
                        self.jungseong = Some(v);
                    }
                    None => {
                        self.flush();
                        self.output.push(raw);
                    }
                }
            }
            (None, ..) => {
                self.flush();
                self.output.push(raw);
            }
        }
    }
}

/// Recompose a run of Hangul jamo/syllables with the standard 2-beolsik
/// automaton. Returns the input unchanged when it contains anything that is
/// not Hangul (jamo or precomposed syllable).
pub fn recompose_hangul_run(run: &str) -> String {
    let units: Option<Vec<Unit>> = run.chars().map(classify).collect();
    let Some(units) = units else {
        return run.to_string();
    };
    let mut automaton = Automaton::default();
    for (idx, unit) in units.iter().enumerate() {
        let next_is_vowel = unit_is_vowel(units.get(idx + 1));
        match *unit {
            Unit::Consonant(l, t, raw) => automaton.consonant(l, t, raw, next_is_vowel),
            Unit::Vowel(v, raw) => automaton.vowel(v, raw),
            Unit::Syllable(l, v, t) => {
                automaton.flush();
                automaton.choseong = Some(l);
                automaton.jungseong = Some(v);
                automaton.jongseong = if t == 0 { None } else { Some(t) };
            }
        }
    }
    automaton.flush();
    automaton.output
}

/// A single lone Hangul jamo (compatibility or conjoining/NFD form).
pub fn is_lone_hangul_jamo(text: &str) -> bool {
    let mut chars = text.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return false;
    };
    matches!(ch as u32, 0x3131..=0x3163 | 0x1100..=0x11FF)
}

fn is_hangul_char(ch: char) -> bool {
    matches!(ch as u32, 0xAC00..=0xD7A3 | 0x3131..=0x3163 | 0x1100..=0x11FF)
}

fn leading_hangul_run(text: &str) -> &str {
    let end = text
        .char_indices()
        .find(|(_, ch)| !is_hangul_char(*ch))
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    &text[..end]
}

/// Buffer edit produced by the repair: replace bytes `start..end` with
/// `replacement`.
#[derive(Debug, PartialEq, Eq)]
pub struct RepairEdit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

/// Tracks a stranded first jamo and decides when to merge it. Feed every
/// KeyDown seen by the editor *after* default handling (positions/compose
/// state settled), plus `flush` on events that end typing (click, focus
/// loss).
#[derive(Default)]
pub struct FirstKeyRepairState {
    stranded_pos: Option<usize>,
}

impl FirstKeyRepairState {
    pub fn reset(&mut self) {
        self.stranded_pos = None;
    }

    /// `compose_state`/`caret` in bytes; `text_range` reads buffer bytes.
    pub fn on_key_event(
        &mut self,
        event_text: &str,
        has_command_modifiers: bool,
        compose_state: usize,
        caret: usize,
        text_range: &dyn Fn(usize, usize) -> Option<String>,
    ) -> Option<RepairEdit> {
        if let Some(pos) = self.stranded_pos {
            if compose_state > 0 {
                let marked_start = match caret.checked_sub(compose_state) {
                    Some(start) if start > pos => start,
                    _ => return None,
                };
                return Self::merge(pos, marked_start, text_range);
            }
            // Composition is not active on this keystroke: the burst ended
            // (or never started). Final merge bounded by the caret, disarm.
            self.stranded_pos = None;
            let edit = Self::merge(pos, caret, text_range);
            if edit.is_some() {
                return edit;
            }
            // fall through so this very event can re-arm below
        }

        if compose_state == 0
            && !has_command_modifiers
            && is_lone_hangul_jamo(event_text)
            && caret >= event_text.len()
            && text_range(caret - event_text.len(), caret).as_deref() == Some(event_text)
        {
            self.stranded_pos = Some(caret - event_text.len());
        }
        None
    }

    /// Final merge + disarm; call when typing is interrupted by a click or
    /// focus change.
    pub fn flush(
        &mut self,
        caret: usize,
        text_range: &dyn Fn(usize, usize) -> Option<String>,
    ) -> Option<RepairEdit> {
        let pos = self.stranded_pos.take()?;
        Self::merge(pos, caret, text_range)
    }

    fn merge(
        pos: usize,
        end: usize,
        text_range: &dyn Fn(usize, usize) -> Option<String>,
    ) -> Option<RepairEdit> {
        if end <= pos {
            return None;
        }
        let committed = text_range(pos, end)?;
        let run = leading_hangul_run(&committed);
        if run.is_empty() {
            return None;
        }
        let merged = recompose_hangul_run(run);
        if merged == run {
            return None;
        }
        Some(RepairEdit {
            start: pos,
            end: pos + run.len(),
            replacement: merged,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recompose_merges_basic_jamo_runs() {
        assert_eq!(recompose_hangul_run("ㅈㅏㅇ"), "장");
        assert_eq!(recompose_hangul_run("ㅈㅏ"), "자");
        assert_eq!(recompose_hangul_run("자ㅇ"), "장");
        assert_eq!(recompose_hangul_run("ㅎㅗㅏㄴ"), "환"); // compound vowel
        assert_eq!(recompose_hangul_run("ㅇㅏㄴㅈ"), "앉"); // double jongseong
        assert_eq!(recompose_hangul_run("ㄱㅏㅅㅏ"), "가사"); // dokkaebibul lookahead
        assert_eq!(recompose_hangul_run("갑ㅅ"), "값");
    }

    #[test]
    fn recompose_leaves_uncombinable_text_alone() {
        assert_eq!(recompose_hangul_run("ㅋㅋㅋ"), "ㅋㅋㅋ");
        assert_eq!(recompose_hangul_run("ㅈ"), "ㅈ");
        assert_eq!(recompose_hangul_run("ㅏ"), "ㅏ");
        assert_eq!(recompose_hangul_run("장영환"), "장영환");
        assert_eq!(recompose_hangul_run("select"), "select");
        assert_eq!(recompose_hangul_run(""), "");
    }

    #[test]
    fn recompose_handles_conjoining_nfd_jamo() {
        // NFD 장 = U+110C U+1161 U+11BC
        assert_eq!(recompose_hangul_run("\u{110C}\u{1161}\u{11BC}"), "장");
    }

    #[test]
    fn repair_replays_the_captured_jang_yeong_hwan_trace() {
        // Buffer contents observed in the verify_ime_minimal trace while
        // typing 장영환 with the broken first key, keyed by KeyDown order.
        let buffer = std::sync::Mutex::new(String::new());
        let read = |start: usize, end: usize| -> Option<String> {
            buffer
                .lock()
                .ok()
                .and_then(|guard| guard.get(start..end).map(str::to_string))
        };

        let mut state = FirstKeyRepairState::default();

        // KeyDown 1: "ㅈ" committed plain, compose_state=0, caret=3.
        *buffer.lock().unwrap() = "ㅈ".to_string();
        assert_eq!(state.on_key_event("ㅈ", false, 0, 3, &read), None);

        // KeyDown 2: marked "ㅏ" at 3..6 — committed region is just "ㅈ".
        *buffer.lock().unwrap() = "ㅈㅏ".to_string();
        assert_eq!(state.on_key_event("ㅏ", false, 3, 6, &read), None);

        // KeyDown 3 (second ㅇ event): buffer ㅈㅏㅇ + marked ㅇ.
        *buffer.lock().unwrap() = "ㅈㅏㅇㅇ".to_string();
        let edit = state
            .on_key_event("ㅏㅇ", false, 3, 12, &read)
            .expect("committed ㅈㅏㅇ merges");
        assert_eq!(
            edit,
            RepairEdit {
                start: 0,
                end: 9,
                replacement: "장".to_string()
            }
        );
        *buffer.lock().unwrap() = "장ㅇ".to_string();

        // KeyDown 4: marked ㅇ→여 at 3..6; committed "장" is already merged.
        *buffer.lock().unwrap() = "장여".to_string();
        assert_eq!(state.on_key_event("여", false, 3, 6, &read), None);

        // Word committed, next keystroke (space) sees compose_state=0:
        // final no-op merge and disarm.
        *buffer.lock().unwrap() = "장영환 ".to_string();
        assert_eq!(state.on_key_event(" ", false, 0, 13, &read), None);
        assert!(state.stranded_pos.is_none());
    }

    #[test]
    fn repair_arms_only_on_the_exact_failure_signature() {
        let buffer = std::sync::Mutex::new(String::new());
        let read = |start: usize, end: usize| -> Option<String> {
            buffer
                .lock()
                .ok()
                .and_then(|guard| guard.get(start..end).map(str::to_string))
        };

        // Healthy composition: first key already marked (compose_state>0).
        let mut healthy = FirstKeyRepairState::default();
        *buffer.lock().unwrap() = "ㅈ".to_string();
        assert_eq!(healthy.on_key_event("ㅈ", false, 3, 3, &read), None);
        assert!(healthy.stranded_pos.is_none());

        // ASCII typing never arms.
        let mut ascii = FirstKeyRepairState::default();
        *buffer.lock().unwrap() = "s".to_string();
        assert_eq!(ascii.on_key_event("s", false, 0, 1, &read), None);
        assert!(ascii.stranded_pos.is_none());

        // Command shortcuts never arm.
        let mut shortcut = FirstKeyRepairState::default();
        *buffer.lock().unwrap() = "ㅈ".to_string();
        assert_eq!(shortcut.on_key_event("ㅈ", true, 0, 3, &read), None);
        assert!(shortcut.stranded_pos.is_none());

        // Buffer content mismatch (event text not actually inserted at the
        // caret, e.g. the key was consumed) never arms.
        let mut mismatch = FirstKeyRepairState::default();
        *buffer.lock().unwrap() = "x".to_string();
        assert_eq!(mismatch.on_key_event("ㅈ", false, 0, 1, &read), None);
        assert!(mismatch.stranded_pos.is_none());
    }

    #[test]
    fn stranded_jamo_without_composition_stays_untouched() {
        // Lone ㅈ then an arrow key / plain keystroke: nothing to merge,
        // state disarms, the jamo the user actually typed is preserved.
        let buffer = std::sync::Mutex::new("ㅈ ".to_string());
        let read = |start: usize, end: usize| -> Option<String> {
            buffer
                .lock()
                .ok()
                .and_then(|guard| guard.get(start..end).map(str::to_string))
        };
        let mut state = FirstKeyRepairState::default();
        assert_eq!(state.on_key_event("ㅈ", false, 0, 3, &read), None);
        assert_eq!(state.on_key_event(" ", false, 0, 4, &read), None);
        assert!(state.stranded_pos.is_none());
    }

    #[test]
    fn reset_discards_stranded_jamo_before_selection_replacement() {
        let buffer = std::sync::Mutex::new("ㅈ다".to_string());
        let read = |start: usize, end: usize| -> Option<String> {
            buffer
                .lock()
                .ok()
                .and_then(|guard| guard.get(start..end).map(str::to_string))
        };
        let mut state = FirstKeyRepairState::default();
        assert_eq!(state.on_key_event("ㅈ", false, 0, 3, &read), None);
        assert!(state.stranded_pos.is_some());

        state.reset();
        *buffer.lock().unwrap() = "한".to_string();
        assert_eq!(state.on_key_event("한", false, 0, 3, &read), None);
        assert!(state.stranded_pos.is_none());
    }

    #[test]
    fn flush_merges_and_disarms() {
        let buffer = std::sync::Mutex::new("ㅈㅏㅇ".to_string());
        let read = |start: usize, end: usize| -> Option<String> {
            buffer
                .lock()
                .ok()
                .and_then(|guard| guard.get(start..end).map(str::to_string))
        };
        let mut state = FirstKeyRepairState {
            stranded_pos: Some(0),
        };
        assert_eq!(
            state.flush(9, &read),
            Some(RepairEdit {
                start: 0,
                end: 9,
                replacement: "장".to_string()
            })
        );
        assert!(state.stranded_pos.is_none());
        assert_eq!(state.flush(9, &read), None);
    }
}
