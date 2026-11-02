//! Exact decimal scaling shared by inventory measurements before float conversion.

/// Convert a signed decimal lexeme to scaled integer units without rounding.
/// Domain owners apply their own bounds. Leading/trailing zeroes and exponents
/// are accepted; whitespace and nonfinite names are not. Eighteen scaled digits
/// cover the inventory measurements without overflowing the signed result.
pub(crate) fn scaled_decimal(raw: &str, scale: u32) -> Option<i64> {
    let (coefficient, exponent) = raw.split_once(['e', 'E']).unwrap_or((raw, "0"));
    let negative = coefficient.starts_with('-');
    let coefficient = coefficient.strip_prefix(['-', '+']).unwrap_or(coefficient);
    let (integer, fraction) = coefficient.split_once('.').unwrap_or((coefficient, ""));
    let exponent_digits = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
    if (integer.is_empty() && fraction.is_empty())
        || !integer
            .bytes()
            .chain(fraction.bytes())
            .all(|ch| ch.is_ascii_digit())
        || exponent_digits.is_empty()
        || !exponent_digits.bytes().all(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    let fraction_len = fraction.len();
    let digits: String = coefficient.chars().filter(|ch| *ch != '.').collect();
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some(0);
    }
    let trimmed = significant.trim_end_matches('0');
    let trailing_zeroes = significant.len() - trimmed.len();
    // Overflowing exponents cannot fit a nonzero measurement's range or scale.
    // Zero with an arbitrarily large exponent is handled above.
    let exponent = exponent.parse::<i64>().unwrap_or_else(|_| {
        if exponent.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        }
    });
    let power = exponent
        .saturating_sub(i64::try_from(fraction_len).ok()?)
        .saturating_add(i64::try_from(trailing_zeroes).ok()?)
        .saturating_add(i64::from(scale));
    if power < 0 || trimmed.len() > 18 || power > 18 - trimmed.len() as i64 {
        return None;
    }
    let magnitude = trimmed
        .parse::<i64>()
        .ok()?
        .checked_mul(10_i64.pow(power as u32))?;
    Some(if negative { -magnitude } else { magnitude })
}
