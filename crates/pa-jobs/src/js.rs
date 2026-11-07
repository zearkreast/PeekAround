//! The few JavaScript number semantics the ffmpeg argument builders depend on.

/// `Number.prototype.toFixed(digits)` — exact decimal value of the double, ties rounded away from
/// zero (JS picks the larger n), unlike Rust's `{:.N}` (ties to even on the exact value).
pub fn to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() {
        return if x.is_nan() {
            "NaN".into()
        } else if x > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if x.abs() >= 1e21 {
        return pa_core::js::num_to_string(x);
    }
    let neg = x < 0.0;
    let ax = x.abs();
    // Rust prints the exact binary value for any precision: with enough digits nothing is rounded
    let exact = format!("{:.*}", 1100, ax);
    let (int, frac) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut digits_v: Vec<u8> = int.bytes().chain(frac.bytes().take(digits)).collect();
    let round_up = frac.as_bytes().get(digits).is_some_and(|&d| d >= b'5');
    if round_up {
        let mut i = digits_v.len();
        loop {
            if i == 0 {
                digits_v.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits_v[i] == b'9' {
                digits_v[i] = b'0';
            } else {
                digits_v[i] += 1;
                break;
            }
        }
    }
    let int_len = digits_v.len() - digits;
    let mut s = String::with_capacity(digits_v.len() + 2);
    if neg {
        s.push('-');
    }
    s.push_str(std::str::from_utf8(&digits_v[..int_len]).unwrap());
    if digits > 0 {
        s.push('.');
        s.push_str(std::str::from_utf8(&digits_v[int_len..]).unwrap());
    }
    s
}

/// Compact decimal for filter args (`f` of video-edit.ts): `toFixed(digits)` without trailing
/// zeros ("0.5", "2", "0.333333"), never "-0".
pub fn f(n: f64, digits: usize) -> String {
    let s = to_fixed(n, digits);
    // /\.?0+$/ — only strip when there is a fraction (toFixed(0) of 100 stays "100")
    let out = if s.contains('.') {
        let t = s.trim_end_matches('0');
        t.strip_suffix('.').unwrap_or(t).to_string()
    } else {
        // JS would strip the zeros of "100" too: /\.?0+$/ matches "00"
        let t = s.trim_end_matches('0');
        if t.is_empty() || t == "-" {
            // "0" → "" in JS as well
            String::new()
        } else {
            t.to_string()
        }
    };
    if out == "-0" {
        "0".into()
    } else {
        out
    }
}

/// `String(n)` of a JS number.
pub fn num(n: f64) -> String {
    pa_core::js::num_to_string(n)
}

/// `Math.round`
pub fn round(x: f64) -> f64 {
    pa_core::js::round(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_like_js() {
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(-0.125, 2), "-0.13");
        assert_eq!(to_fixed(1.005, 2), "1.00"); // 1.00499999999999989…
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(9.9999999, 3), "10.000");
        assert_eq!(to_fixed(-1e-7, 6), "-0.000000");
        assert_eq!(to_fixed(1.0 / 3.0, 6), "0.333333");
        assert_eq!(f(0.5, 6), "0.5");
        assert_eq!(f(2.0, 6), "2");
        assert_eq!(f(100.0, 3), "100");
        assert_eq!(f(-1e-7, 6), "0");
        assert_eq!(f(1.0 / 3.0, 6), "0.333333");
        assert_eq!(f(0.0, 4), "0");
    }
}
