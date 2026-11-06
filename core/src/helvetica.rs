use crate::NOTE_FONT_SIZE;

/// Helvetica advance widths by WinAnsi code, from the Adobe core font metrics.
const WIDTHS: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
    0, 556, 0, 222, 556, 333, 1000, 556, 556, 333, 1000, 667, 333, 1000, 0, 611, 0, 0, 222, 222,
    333, 333, 350, 556, 1000, 333, 1000, 500, 333, 944, 0, 500, 500, 278, 333, 556, 556, 556, 556,
    260, 556, 333, 737, 370, 556, 584, 333, 737, 333, 400, 584, 333, 333, 333, 556, 537, 278, 333,
    333, 365, 556, 834, 834, 834, 611, 667, 667, 667, 667, 667, 667, 1000, 722, 667, 667, 667, 667,
    278, 278, 278, 278, 722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667,
    611, 556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278, 556, 556,
    556, 556, 556, 556, 556, 584, 611, 556, 556, 556, 556, 500, 556, 500,
];

/// The WinAnsi byte for `character`, if Helvetica can draw it.
fn win_ansi(character: char) -> Option<u8> {
    let code = u32::from(character);
    let byte = match code {
        0x20..=0x7e | 0xa0..=0xff => code as u8,
        0x152 => 140,
        0x153 => 156,
        0x160 => 138,
        0x161 => 154,
        0x178 => 159,
        0x17d => 142,
        0x17e => 158,
        0x192 => 131,
        0x2c6 => 136,
        0x2dc => 152,
        0x2013 => 150,
        0x2014 => 151,
        0x2018 => 145,
        0x2019 => 146,
        0x201a => 130,
        0x201c => 147,
        0x201d => 148,
        0x201e => 132,
        0x2020 => 134,
        0x2021 => 135,
        0x2022 => 149,
        0x2026 => 133,
        0x2030 => 137,
        0x2039 => 139,
        0x203a => 155,
        0x20ac => 128,
        0x2122 => 153,
        _ => return None,
    };
    Some(byte)
}

/// Encodes `text` for the built-in Helvetica font, or `None` if a character has no glyph.
pub fn encode_win_ansi(text: &str) -> Option<Vec<u8>> {
    text.chars().map(win_ansi).collect()
}

fn width(text: &str) -> f64 {
    let units: u32 = text
        .chars()
        .filter_map(win_ansi)
        .map(|byte| u32::from(WIDTHS[usize::from(byte)]))
        .sum();
    f64::from(units) * NOTE_FONT_SIZE / 1000.0
}

/// Wraps a note to `max_width` points, breaking inside words that cannot fit a line.
pub fn wrap_text_note(text: &str, max_width: f64) -> Result<String, String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if encode_win_ansi(word).is_none() {
            return Err("Text notes currently support Western European characters. Other writing systems need an embedded font.".into());
        }
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if width(&candidate) <= max_width {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        for character in word.chars() {
            let mut grown = line.clone();
            grown.push(character);
            if !line.is_empty() && width(&grown) > max_width {
                lines.push(std::mem::take(&mut line));
                line.push(character);
            } else {
                line = grown;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_lines_keep_every_character_and_fit() {
        for text in [
            "AV To résumé café",
            "one two three",
            "unbreakablelongword",
            "a",
            "",
        ] {
            let wrapped = wrap_text_note(text, 80.0).unwrap();
            assert_eq!(wrapped.replace(['\n', ' '], ""), text.replace(' ', ""));
            assert!(wrapped.lines().all(|line| width(line) <= 80.0));
        }
    }

    #[test]
    fn western_text_wraps_and_other_scripts_are_rejected() {
        assert!(
            wrap_text_note("Résumé café and a long thought about reading", 80.0)
                .unwrap()
                .contains('\n')
        );
        let error = wrap_text_note("你好", 100.0).unwrap_err();
        assert!(error.contains("Western European") && error.contains("embedded font"));
    }

    #[test]
    fn typographic_punctuation_uses_its_win_ansi_slot() {
        assert_eq!(encode_win_ansi("“€”"), Some(vec![147, 128, 148]));
    }

    #[test]
    fn helvetica_metrics_match_the_core_font() {
        assert_eq!(width("A"), 667.0 * 12.0 / 1000.0);
    }
}
