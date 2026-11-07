//! Human-facing decimal token units. Stored and transported counts remain integers.

pub fn format_tokens(tokens: u64) -> String {
    let whole = tokens / 1000;
    let fraction = tokens % 1000;
    if fraction == 0 {
        format!("{whole}k")
    } else {
        let fraction = format!("{fraction:03}");
        format!("{whole}.{}k", fraction.trim_end_matches('0'))
    }
}

#[cfg(test)]
mod tests {
    use super::format_tokens;

    #[test]
    fn decimal_units_preserve_token_precision_without_switching_units() {
        for (tokens, expected) in [
            (0, "0k"),
            (1, "0.001k"),
            (999, "0.999k"),
            (1000, "1k"),
            (1024, "1.024k"),
            (1248, "1.248k"),
            (8192, "8.192k"),
            (128000, "128k"),
            (131072, "131.072k"),
            (1000000, "1000k"),
            (u64::MAX, "18446744073709551.615k"),
        ] {
            assert_eq!(format_tokens(tokens), expected);
        }
    }
}
