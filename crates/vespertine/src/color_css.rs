//! Shared CSS color text parser (THM-001).
//!
//! Every place that accepts a color as text goes through [`parse_css_color`]. The parser
//! understands named colors, hexadecimal forms, the legacy and modern functional notations for
//! sRGB, HSL, HWB, HSV, CIE Lab/LCH and OKLab/OKLCH, and the relative-color form with a small
//! one-operation `calc()` sub-language. Whatever the notation, the result is an sRGB color
//! whose four channels are hard-clamped to `[0, 1]` (no gamut mapping).
//!
//! All arithmetic is done in 32-bit floating point. Nesting of relative colors and `calc`
//! groups is limited to [`MAX_NESTING`] levels so that adversarial input cannot exhaust the
//! stack; deeper input is invalid.

use crate::Rgba;

/// Maximum depth of nested relative colors and parenthesized `calc` groups.
const MAX_NESTING: usize = 64;

/// Parses CSS color text into a four-channel sRGB tuple `[r, g, b, a]`, each in `[0, 1]`.
///
/// Returns `None` for invalid input; no substitute color is ever produced.
pub fn parse_css_color(text: &str) -> Option<[f32; 4]> {
    let [r, g, b, a] = parse_color(text, 0)?;
    if [r, g, b, a].iter().any(|c| c.is_nan()) {
        return None;
    }
    Some([
        r.clamp(0., 1.),
        g.clamp(0., 1.),
        b.clamp(0., 1.),
        a.clamp(0., 1.),
    ])
}

/// Parses CSS color text into the sRGB paint type. See [`parse_css_color`].
pub fn css_color_to_rgba(text: &str) -> Option<Rgba> {
    let [r, g, b, a] = parse_css_color(text)?;
    Some(Rgba { r, g, b, a })
}

/// Parses CSS color text into the packed `r << 24 | g << 16 | b << 8 | a` form, each field
/// being the channel multiplied by 255 and truncated toward zero. See [`parse_css_color`].
pub fn css_color_to_packed(text: &str) -> Option<u32> {
    css_color_to_rgba(text).map(u32::from)
}

/// Converts an OKLCH color (lightness 0..1, chroma, hue in degrees) to sRGB with the given
/// alpha: OKLab matrices, linear channels clipped to `[0, 1]`, then sRGB encoding (THM-002 R9).
pub fn oklch_to_rgba(lightness: f32, chroma: f32, hue_degrees: f32, alpha: f32) -> Rgba {
    let hue = hue_degrees.to_radians();
    let [r, g, b] = oklab_to_linear(lightness, chroma * hue.cos(), chroma * hue.sin())
        .map(|c| srgb_encode(c.clamp(0., 1.)));
    Rgba { r, g, b, a: alpha }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Notation {
    Rgb,
    Hsl,
    Hwb,
    Hsv,
    Lab,
    Lch,
    Oklab,
    Oklch,
}

impl Notation {
    fn from_function_name(name: &str) -> Option<Self> {
        const NAMES: [(&str, Notation); 12] = [
            ("rgb", Notation::Rgb),
            ("rgba", Notation::Rgb),
            ("hsl", Notation::Hsl),
            ("hsla", Notation::Hsl),
            ("hwb", Notation::Hwb),
            ("hwba", Notation::Hwb),
            ("hsv", Notation::Hsv),
            ("hsva", Notation::Hsv),
            ("lab", Notation::Lab),
            ("lch", Notation::Lch),
            ("oklab", Notation::Oklab),
            ("oklch", Notation::Oklch),
        ];
        NAMES
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|&(_, notation)| notation)
    }

    /// Names of the three channel variables of the relative notation.
    fn variable_names(self) -> [&'static str; 3] {
        match self {
            Notation::Rgb => ["r", "g", "b"],
            Notation::Hsl => ["h", "s", "l"],
            Notation::Hwb => ["h", "w", "b"],
            Notation::Hsv => ["h", "s", "v"],
            Notation::Lab | Notation::Oklab => ["l", "a", "b"],
            Notation::Lch | Notation::Oklch => ["l", "c", "h"],
        }
    }
}

/// Parses without the final clamp (relative bases keep out-of-range channels).
fn parse_color(text: &str, depth: usize) -> Option<[f32; 4]> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("transparent") {
        return Some([0.; 4]);
    }
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(open) = text.find('(')
        && text.ends_with(')')
    {
        let notation = Notation::from_function_name(text[..open].trim_end())?;
        let args = &text[open + 1..text.len() - 1];
        return parse_absolute(notation, args).or_else(|| {
            if text.is_ascii() {
                parse_relative(notation, args, depth)
            } else {
                None
            }
        });
    }
    parse_hex(text).or_else(|| named_color(text))
}

fn parse_hex(hex: &str) -> Option<[f32; 4]> {
    let digits = hex.as_bytes();
    if !matches!(digits.len(), 3 | 4 | 6 | 8) || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let nibble = |i: usize| (digits[i] as char).to_digit(16).unwrap_or(0) as u8;
    let mut channels = [255u8; 4];
    if digits.len() <= 4 {
        for (i, channel) in channels.iter_mut().enumerate().take(digits.len()) {
            *channel = nibble(i) * 17;
        }
    } else {
        for (i, channel) in channels.iter_mut().enumerate().take(digits.len() / 2) {
            *channel = nibble(2 * i) * 16 + nibble(2 * i + 1);
        }
    }
    Some(channels.map(|c| c as f32 / 255.))
}

fn named_color(name: &str) -> Option<[f32; 4]> {
    if !(3..=20).contains(&name.len()) || !name.is_ascii() {
        return None;
    }
    let mut lower = [0u8; 20];
    for (dst, src) in lower.iter_mut().zip(name.bytes()) {
        *dst = src.to_ascii_lowercase();
    }
    let lower = &lower[..name.len()];
    let index = NAMED_COLORS
        .binary_search_by(|(candidate, _)| candidate.as_bytes().cmp(lower))
        .ok()?;
    let rgb = NAMED_COLORS[index].1;
    Some([
        (rgb >> 16 & 0xff) as f32 / 255.,
        (rgb >> 8 & 0xff) as f32 / 255.,
        (rgb & 0xff) as f32 / 255.,
        1.,
    ])
}

/// Cursor over ASCII argument text that produces R7 value tokens.
struct Scanner<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.text.len()
    }

    /// Skips U+0020 characters (the only separator) and returns how many were skipped.
    fn skip_spaces(&mut self) -> usize {
        let start = self.pos;
        while self.peek() == Some(b' ') {
            self.pos += 1;
        }
        self.pos - start
    }

    /// Reads a non-empty value token: up to the next space, comma or slash outside any nested
    /// parentheses.
    fn token(&mut self) -> Option<&'a str> {
        let start = self.pos;
        let mut depth = 0usize;
        while let Some(byte) = self.peek() {
            match byte {
                b'(' => depth += 1,
                b')' => depth = depth.saturating_sub(1),
                b' ' | b',' | b'/' if depth == 0 => break,
                _ => {}
            }
            self.pos += 1;
        }
        (self.pos > start).then(|| &self.text[start..self.pos])
    }

    /// Consumes a separator gap between value tokens: spaces and at most one comma.
    fn gap(&mut self) -> Option<()> {
        let mut consumed = self.skip_spaces();
        if self.peek() == Some(b',') {
            self.pos += 1;
            consumed += 1;
            self.skip_spaces();
        }
        (consumed > 0).then_some(())
    }
}

/// A parsed value token of the absolute notation.
#[derive(Clone, Copy)]
enum Value {
    Number(f32),
    /// A percentage, already divided by 100.
    Percent(f32),
}

impl Value {
    /// `none` evaluates to zero in every channel position.
    fn parse(token: &str) -> Option<Self> {
        if token.eq_ignore_ascii_case("none") {
            return Some(Value::Number(0.));
        }
        let (number, rest) = scan_number(token)?;
        match rest {
            "" => Some(Value::Number(number)),
            "%" => Some(Value::Percent(number / 100.)),
            _ => None,
        }
    }

    /// Plain number or percentage with the percentage scaled by `percent_scale`.
    fn scaled(self, plain_divisor: f32, percent_scale: f32) -> f32 {
        match self {
            Value::Number(number) => number / plain_divisor,
            Value::Percent(fraction) => fraction * percent_scale,
        }
    }
}

/// Parses an angle in degrees: `none`, or a number with an optional unit.
fn parse_angle(token: &str) -> Option<f32> {
    if token.eq_ignore_ascii_case("none") {
        return Some(0.);
    }
    let (number, unit) = scan_number(token)?;
    if unit.is_empty() || unit.eq_ignore_ascii_case("deg") {
        Some(number)
    } else if unit.eq_ignore_ascii_case("grad") {
        Some(number * 360. / 400.)
    } else if unit.eq_ignore_ascii_case("rad") {
        Some(number.to_degrees())
    } else if unit.eq_ignore_ascii_case("turn") {
        Some(number * 360.)
    } else {
        None
    }
}

fn parse_alpha(token: &str) -> Option<f32> {
    Some(Value::parse(token)?.scaled(1., 1.).clamp(0., 1.))
}

/// Scans a decimal floating-point literal at the start of `text` (optional sign, digits with
/// optional fraction, optional exponent) and returns its 32-bit value and the remaining text.
/// Non-finite spellings and non-finite results are rejected.
fn scan_number(text: &str) -> Option<(f32, &str)> {
    let bytes = text.as_bytes();
    let digits_at = |mut i: usize| {
        let start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        (i, i - start)
    };
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let (after_int, int_digits) = digits_at(end);
    end = after_int;
    let mut fraction_digits = 0;
    if bytes.get(end) == Some(&b'.') {
        let (after_fraction, count) = digits_at(end + 1);
        end = after_fraction;
        fraction_digits = count;
    }
    if int_digits + fraction_digits == 0 {
        return None;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let (after_exponent, count) = digits_at(end + 1 + sign);
        if count > 0 {
            end = after_exponent;
        }
    }
    let value: f32 = text[..end].parse().ok()?;
    value.is_finite().then(|| (value, &text[end..]))
}

fn parse_absolute(notation: Notation, args: &str) -> Option<[f32; 4]> {
    if !args.is_ascii() {
        return None;
    }
    let mut scanner = Scanner::new(args);
    scanner.skip_spaces();
    let first = scanner.token()?;
    scanner.gap()?;
    let second = scanner.token()?;
    scanner.gap()?;
    let third = scanner.token()?;
    let spaces = scanner.skip_spaces();
    let alpha = if scanner.at_end() {
        1.
    } else {
        if matches!(scanner.peek(), Some(b',' | b'/')) {
            scanner.pos += 1;
            scanner.skip_spaces();
        } else if spaces == 0 {
            return None;
        }
        let alpha = scanner.token()?;
        scanner.skip_spaces();
        if !scanner.at_end() {
            return None;
        }
        parse_alpha(alpha)?
    };

    let channels = match notation {
        Notation::Rgb => [first, second, third]
            .map(|token| Value::parse(token).map(|value| value.scaled(255., 1.).clamp(0., 1.))),
        Notation::Hsl | Notation::Hwb | Notation::Hsv => [
            parse_angle(first),
            Value::parse(second).map(|value| value.scaled(1., 1.)),
            Value::parse(third).map(|value| value.scaled(1., 1.)),
        ],
        Notation::Lab => [
            Value::parse(first).map(|value| value.scaled(1., 100.)),
            Value::parse(second).map(|value| value.scaled(1., 125.)),
            Value::parse(third).map(|value| value.scaled(1., 125.)),
        ],
        Notation::Lch => [
            Value::parse(first).map(|value| value.scaled(1., 100.)),
            Value::parse(second).map(|value| value.scaled(1., 150.)),
            parse_angle(third),
        ],
        Notation::Oklab => [
            Value::parse(first).map(|value| value.scaled(1., 1.)),
            Value::parse(second).map(|value| value.scaled(1., 0.4)),
            Value::parse(third).map(|value| value.scaled(1., 0.4)),
        ],
        Notation::Oklch => [
            Value::parse(first).map(|value| value.scaled(1., 1.)),
            Value::parse(second).map(|value| value.scaled(1., 0.4)),
            parse_angle(third),
        ],
    };
    let [c1, c2, c3] = channels;
    let [r, g, b] = convert(notation, [c1?, c2?, c3?]);
    Some([r, g, b, alpha])
}

/// Converts notation channels to encoded (unclamped) sRGB.
///
/// Inputs: rgb as fractions; hsl/hwb/hsv as (hue degrees, fraction, fraction); lab/oklab as
/// (L, a, b); lch/oklch as (L, C, hue degrees). Fractions, lightness and chroma are limited
/// here as the notations require.
fn convert(notation: Notation, [c1, c2, c3]: [f32; 3]) -> [f32; 3] {
    match notation {
        Notation::Rgb => [c1, c2, c3],
        Notation::Hsl => hsl_to_rgb(normalize_hue(c1), c2.clamp(0., 1.), c3.clamp(0., 1.)),
        Notation::Hwb => hwb_to_rgb(normalize_hue(c1), c2.clamp(0., 1.), c3.clamp(0., 1.)),
        Notation::Hsv => hsv_to_rgb(normalize_hue(c1), c2.clamp(0., 1.), c3.clamp(0., 1.)),
        Notation::Lab => lab_to_srgb(c1.max(0.), c2, c3),
        Notation::Lch => {
            let hue = c3.to_radians();
            lab_to_srgb(c1.max(0.), c2.max(0.) * hue.cos(), c2.max(0.) * hue.sin())
        }
        Notation::Oklab => oklab_to_srgb(c1.max(0.), c2, c3),
        Notation::Oklch => {
            let hue = c3.to_radians();
            oklab_to_srgb(c1.max(0.), c2.max(0.) * hue.cos(), c2.max(0.) * hue.sin())
        }
    }
}

fn normalize_hue(hue: f32) -> f32 {
    ((hue % 360.) + 360.) % 360.
}

/// sRGB transfer, linear to encoded (R14). Applies to out-of-range values as well.
fn srgb_encode(linear: f32) -> f32 {
    if linear >= 0.003_130_8 {
        1.055 * linear.powf(1. / 2.4) - 0.055
    } else {
        12.92 * linear
    }
}

/// sRGB transfer, encoded to linear (R14).
fn srgb_decode(encoded: f32) -> f32 {
    if encoded >= 0.040_45 {
        ((encoded + 0.055) / 1.055).powf(2.4)
    } else {
        encoded / 12.92
    }
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> [f32; 3] {
    if saturation == 0. {
        return [lightness; 3];
    }
    let n2 = if lightness < 0.5 {
        lightness * (1. + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let n1 = 2. * lightness - n2;
    let channel = |t: f32| {
        let t = ((t % 6.) + 6.) % 6.;
        if t < 1. {
            n1 + (n2 - n1) * t
        } else if t < 3. {
            n2
        } else if t < 4. {
            n1 + (n2 - n1) * (4. - t)
        } else {
            n1
        }
    };
    let h = hue / 60.;
    [channel(h + 2.), channel(h), channel(h - 2.)]
}

fn hwb_to_rgb(hue: f32, whiteness: f32, blackness: f32) -> [f32; 3] {
    if whiteness + blackness >= 1. {
        return [whiteness / (whiteness + blackness); 3];
    }
    hsl_to_rgb(hue, 1., 0.5).map(|c| c * (1. - whiteness - blackness) + whiteness)
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
    let lightness = (2. - saturation) * value / 2.;
    let hsl_saturation = if lightness == 0. {
        saturation
    } else if lightness == 1. {
        0.
    } else if lightness < 0.5 {
        saturation * value / (2. * lightness)
    } else {
        saturation * value / (2. - 2. * lightness)
    };
    hsl_to_rgb(hue, hsl_saturation, lightness)
}

/// OKLab to encoded sRGB (R18 + R14), not clamped.
fn oklab_to_srgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    oklab_to_linear(l, a, b).map(srgb_encode)
}

/// OKLab to linear sRGB (R18), not clamped.
fn oklab_to_linear(l: f32, a: f32, b: f32) -> [f32; 3] {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_35 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3,
        -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_4 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3,
    ]
}

/// Encoded sRGB to OKLab.
fn srgb_to_oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = [r, g, b].map(srgb_decode);
    let l_ = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m_ = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s_ = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    ]
}

const LAB_DELTA: f32 = 6. / 29.;
const LAB_WHITE: [f32; 3] = [0.95047, 1.0, 1.08883];

/// CIE Lab (D65 white, no adaptation) to encoded sRGB (R19 + R14), not clamped.
fn lab_to_srgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let fy = (l + 16.) / 116.;
    let fx = fy + a / 500.;
    let fz = fy - b / 200.;
    let inverse = |t: f32| {
        if t > LAB_DELTA {
            t * t * t
        } else {
            (t - 16. / 116.) * 3. * LAB_DELTA * LAB_DELTA
        }
    };
    let x = LAB_WHITE[0] * inverse(fx);
    let y = LAB_WHITE[1] * inverse(fy);
    let z = LAB_WHITE[2] * inverse(fz);
    [
        3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z,
        -0.969_266 * x + 1.876_010_8 * y + 0.041_556 * z,
        0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z,
    ]
    .map(srgb_encode)
}

/// Encoded sRGB to CIE Lab (D65 white, no adaptation).
fn srgb_to_lab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(srgb_decode);
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b;
    let forward = |t: f32| {
        if t > LAB_DELTA * LAB_DELTA * LAB_DELTA {
            t.cbrt()
        } else {
            t / (3. * LAB_DELTA * LAB_DELTA) + 4. / 29.
        }
    };
    let fx = forward(x / LAB_WHITE[0]);
    let fy = forward(y / LAB_WHITE[1]);
    let fz = forward(z / LAB_WHITE[2]);
    [116. * fy - 16., 500. * (fx - fy), 200. * (fy - fz)]
}

/// Cartesian (a, b) to (chroma, hue degrees in [0, 360)).
fn to_polar(a: f32, b: f32) -> (f32, f32) {
    (a.hypot(b), normalize_hue(b.atan2(a).to_degrees()))
}

/// (hue degrees, saturation, lightness) of an encoded sRGB color whose channels are already in
/// [0, 1].
fn rgb_to_hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.;
    let delta = max - min;
    if delta == 0. {
        return (0., 0., lightness);
    }
    let saturation = if lightness < 0.5 {
        delta / (max + min)
    } else {
        delta / (2. - max - min)
    };
    let sector = if max == r {
        ((g - b) / delta) % 6.
    } else if max == g {
        (b - r) / delta + 2.
    } else {
        (r - g) / delta + 4.
    };
    (normalize_hue(sector * 60.), saturation, lightness)
}

/// The base color expressed in the target notation's three variables (R21).
fn base_variables(notation: Notation, base: [f32; 4]) -> [f32; 3] {
    let rgb = [base[0], base[1], base[2]];
    let clamped = rgb.map(|c| c.clamp(0., 1.));
    let max = clamped[0].max(clamped[1]).max(clamped[2]);
    let min = clamped[0].min(clamped[1]).min(clamped[2]);
    match notation {
        Notation::Rgb => rgb.map(|c| c * 255.),
        Notation::Hsl => {
            let (h, s, l) = rgb_to_hsl(clamped);
            [h, s * 100., l * 100.]
        }
        Notation::Hwb => [rgb_to_hsl(clamped).0, min * 100., (1. - max) * 100.],
        Notation::Hsv => {
            let saturation = if max == 0. { 0. } else { (max - min) / max };
            [rgb_to_hsl(clamped).0, saturation * 100., max * 100.]
        }
        Notation::Lab => srgb_to_lab(rgb),
        Notation::Lch => {
            let [l, a, b] = srgb_to_lab(rgb);
            let (chroma, hue) = to_polar(a, b);
            [l, chroma, hue]
        }
        Notation::Oklab => srgb_to_oklab(rgb),
        Notation::Oklch => {
            let [l, a, b] = srgb_to_oklab(rgb);
            let (chroma, hue) = to_polar(a, b);
            [l, chroma, hue]
        }
    }
}

/// Variables available while evaluating relative-color component tokens.
struct Variables {
    names: [&'static str; 3],
    values: [f32; 3],
    alpha: f32,
}

impl Variables {
    fn lookup(&self, name: &str) -> Option<f32> {
        if name.eq_ignore_ascii_case("alpha") {
            return Some(self.alpha);
        }
        self.names
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(name))
            .map(|index| self.values[index])
    }

    /// Evaluates one component token: a plain number, a variable or a `calc(...)` expression.
    fn evaluate(&self, token: &str) -> Option<f32> {
        let bytes = token.as_bytes();
        if bytes.len() > 5
            && bytes[..4].eq_ignore_ascii_case(b"calc")
            && bytes[4] == b'('
            && bytes[bytes.len() - 1] == b')'
        {
            let mut calc = Calc {
                text: &token[5..token.len() - 1],
                pos: 0,
                variables: self,
            };
            let value = calc.expression(0)?;
            calc.skip_spaces();
            return (calc.pos == calc.text.len() && value.is_finite()).then_some(value);
        }
        if let Some((number, "")) = scan_number(token) {
            return Some(number);
        }
        self.lookup(token)
    }
}

/// Evaluator for the inside of `calc(...)`: exactly one binary operation per group.
struct Calc<'a> {
    text: &'a str,
    pos: usize,
    variables: &'a Variables,
}

impl Calc<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn skip_spaces(&mut self) {
        while self.peek() == Some(b' ') {
            self.pos += 1;
        }
    }

    /// `operand operator operand` with optional spaces around each part.
    fn expression(&mut self, depth: usize) -> Option<f32> {
        let left = self.operand(depth)?;
        self.skip_spaces();
        let operator = self.peek()?;
        self.pos += 1;
        let right = self.operand(depth)?;
        let result = match operator {
            b'+' => left + right,
            b'-' => left - right,
            b'*' => left * right,
            b'/' if right != 0. => left / right,
            _ => return None,
        };
        result.is_finite().then_some(result)
    }

    fn operand(&mut self, depth: usize) -> Option<f32> {
        self.skip_spaces();
        if self.peek() == Some(b'(') {
            if depth >= MAX_NESTING {
                return None;
            }
            self.pos += 1;
            let value = self.expression(depth + 1)?;
            self.skip_spaces();
            if self.peek() != Some(b')') {
                return None;
            }
            self.pos += 1;
            return Some(value);
        }
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(byte) = self.peek() {
            if matches!(byte, b'+' | b'-' | b'*' | b'/' | b' ' | b'(' | b')') {
                break;
            }
            self.pos += 1;
        }
        let operand = &self.text[start..self.pos];
        let literal = operand.strip_prefix('-').unwrap_or(operand);
        if literal.bytes().any(|b| b.is_ascii_digit())
            && literal.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        {
            return operand.parse().ok();
        }
        if operand.bytes().all(|b| b.is_ascii_alphabetic()) {
            return self.variables.lookup(operand);
        }
        None
    }
}

/// `fn( from <base> v1 v2 v3 [ / alpha ] )` (R20..R23).
fn parse_relative(notation: Notation, args: &str, depth: usize) -> Option<[f32; 4]> {
    if depth >= MAX_NESTING {
        return None;
    }
    let mut scanner = Scanner::new(args);
    scanner.skip_spaces();
    if !scanner.token()?.eq_ignore_ascii_case("from") || scanner.skip_spaces() == 0 {
        return None;
    }
    let base = parse_color(scanner.token()?, depth + 1)?;
    let variables = Variables {
        names: notation.variable_names(),
        values: base_variables(notation, base),
        alpha: base[3],
    };
    let mut components = [0f32; 3];
    for component in &mut components {
        if scanner.skip_spaces() == 0 {
            return None;
        }
        *component = variables.evaluate(scanner.token()?)?;
    }
    scanner.skip_spaces();
    let alpha = if scanner.at_end() {
        base[3]
    } else {
        if scanner.peek() != Some(b'/') {
            return None;
        }
        scanner.pos += 1;
        scanner.skip_spaces();
        let alpha = variables.evaluate(scanner.token()?)?;
        scanner.skip_spaces();
        if !scanner.at_end() {
            return None;
        }
        alpha
    };

    let [v1, v2, v3] = components;
    let channels = match notation {
        Notation::Rgb => [v1 / 255., v2 / 255., v3 / 255.],
        Notation::Hsl | Notation::Hwb | Notation::Hsv => [v1, v2 / 100., v3 / 100.],
        Notation::Lab | Notation::Lch | Notation::Oklab | Notation::Oklch => [v1, v2, v3],
    };
    let [r, g, b] = convert(notation, channels);
    Some([r, g, b, alpha])
}

/// The 148 CSS Color Module Level 4 named colors as 0xRRGGBB, sorted by name for binary search.
const NAMED_COLORS: [(&str, u32); 148] = [
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn color(text: &str) -> [f32; 4] {
        parse_css_color(text).unwrap_or_else(|| panic!("`{text}` should parse"))
    }

    fn assert_close(actual: [f32; 4], expected: [f32; 4], tolerance: f32, label: &str) {
        for (a, e) in actual.iter().zip(expected) {
            assert!(
                (a - e).abs() <= tolerance,
                "{label}: {actual:?} differs from {expected:?}"
            );
        }
    }

    fn assert_same(text: &str, expected: &str) {
        assert_close(color(text), color(expected), 1. / 255., text);
    }

    fn eight_bit(text: &str) -> [u8; 4] {
        color(text).map(|c| (c * 255.).round() as u8)
    }

    #[test]
    fn named_color_table_is_complete_sorted_and_unique() {
        assert_eq!(NAMED_COLORS.len(), 148);
        assert!(NAMED_COLORS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        for (name, rgb) in NAMED_COLORS {
            assert!((3..=20).contains(&name.len()), "{name}");
            let [r, g, b, a] = eight_bit(name);
            assert_eq!(
                [r, g, b, a],
                [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255],
                "{name}"
            );
        }
        assert_eq!(eight_bit("rebeccapurple"), [0x66, 0x33, 0x99, 255]);
        assert_eq!(color("gray"), color("grey"));
        assert_eq!(color("lightslategray"), color("lightslategrey"));
    }

    #[test]
    fn equivalence_table() {
        assert_same("#f00f", "#ff0000ff");
        assert_same("ff0000ff", "#ff0000ff");
        assert_same("rebeccapurple", "#663399");
        assert_same("transparent", "#00000000");
        for text in [
            "rgb(255 0 0)",
            "rgba(255, 0, 0, 1)",
            "hsl(0 100% 50%)",
            "hsla(0, 100%, 50%, 1)",
            "hwb(0 0% 0%)",
            "hwba(0, 0%, 0%, 1)",
            "hsv(0 100% 100%)",
            "hsva(0, 100%, 100%, 1)",
            "oklab(0.62796 0.22486 0.12585)",
            "oklch(0.62796 0.25768 29.23388)",
        ] {
            assert_same(text, "#ff0000");
        }
        assert_same("lab(100% 0 0)", "#ffffff");
        assert_same("lch(100% 0 0)", "#ffffff");
        assert_same("rgb(none none none / none)", "#00000000");
    }

    #[test]
    fn alpha_forms() {
        for text in [
            "oklch(0 0 0 / 50%)",
            "oklab(0 0 0 / 50%)",
            "hsva(0, 0%, 0%, 0.5)",
            "rgb(0 0 0 / 50%)",
            "lch(0% 0 0 / 50%)",
            "hwba(0, 0%, 100%, 0.5)",
            "rgba(0, 0, 0, 0.5)",
            "lab(0% 0 0 / 50%)",
            "hsl(0 0% 0% / 50%)",
            "hsv(0 0% 0% / 50%)",
            "hsla(0, 0%, 0%, 0.5)",
            "hwb(0 0% 100% / 50%)",
        ] {
            assert_eq!(color(text)[3], 0.5, "{text}");
        }
        for text in ["rgb(1 2 3 0.5)", "rgb(1,2,3/0.5)", "rgb(1 2 3 , 0.5)"] {
            assert_eq!(color(text)[3], 0.5, "{text}");
        }
        for text in [
            "rgb(1,,2,3)",
            "rgb(1\t2\t3)",
            "rgb(1\n2\n3)",
            "rgb(1 2 3 / )",
        ] {
            assert!(parse_css_color(text).is_none(), "{text}");
        }
        assert_eq!(color("rgb(1 2 3 / 300%)")[3], 1.);
        assert_eq!(color("rgb(1 2 3 / -1)")[3], 0.);
        assert!(parse_css_color("rgb(1 2 3 / 0.5deg)").is_none());
        assert!(parse_css_color("rgb(1 2 3 / x)").is_none());
    }

    #[test]
    fn relative_forms() {
        assert_same("rgb(from #bad455 b r g / alpha)", "#55bad4");
        for text in [
            "hsl(from #bad455 h s l / alpha)",
            "hwb(from #bad455 h w b / alpha)",
            "hsv(from #bad455 h s v / alpha)",
            "lab(from #bad455 l a b / alpha)",
            "lch(from #bad455 l c h / alpha)",
        ] {
            assert_same(text, "#bad455");
        }
        assert_close(
            color("oklab(from #bad455 calc(l * 0.7) a b)"),
            color("#708500"),
            1.5 / 255.,
            "oklab calc",
        );
        assert_close(
            color("oklch(from #bad455 calc(l - 0.15) calc(c * 0.7) h)"),
            color("#8fa150"),
            1.5 / 255.,
            "oklch calc",
        );
        assert_eq!(color("lab(from #bad455 l a b / calc(alpha / 2))")[3], 0.5);
        assert_eq!(color("lch(from #bad455 l c h / calc(alpha * 0.5))")[3], 0.5);
        assert_same("RGB(FROM #bad455 B R G / ALPHA)", "#55bad4");
        assert_same("rgb(from #bad455 b r g/alpha)", "#55bad4");
        assert_same("rgb(from rgb(from #bad455 b r g) b r g)", "#d455ba");
        assert_same("rgb(from rgb(186, 212, 85) r g b)", "#bad455");
        assert_eq!(color("rgb(from #bad455 r g b / 0.25)")[3], 0.25);
        assert_same("rgba(from #bad455 255 0 0)", "#ff0000");
        for text in [
            "rgb(from #bad455 r g b / 50%)",
            "rgb(from #bad455 r, g, b)",
            "rgb(from #bad455 r g)",
            "rgb(from #bad455 r g b b)",
            "rgb(from #bad455 50% g b)",
            "rgb(from #bad455 none g b)",
            "rgb(from #bad455 h g b)",
            "rgb(from nope r g b)",
            "rgb(from#bad455 r g b)",
            "foo(from #bad455 r g b)",
        ] {
            assert!(parse_css_color(text).is_none(), "{text}");
        }
    }

    fn calc(expression: &str) -> Option<f32> {
        let variables = Variables {
            names: ["r", "g", "b"],
            values: [255., 127., 0.],
            alpha: 1.,
        };
        variables.evaluate(expression)
    }

    #[test]
    fn calc_language() {
        assert_eq!(calc("calc(4+5.5)"), Some(9.5));
        assert_eq!(calc("calc( 10 - 7 )"), Some(3.));
        assert_eq!(calc("CALC(2.5 *2)"), Some(5.));
        assert_eq!(calc("calc(-97+-18)"), Some(-115.));
        assert_eq!(calc("calc(100--35)"), Some(135.));
        assert_eq!(calc("calc((r + g) / 2)"), Some(191.));
        assert_eq!(calc("calc((2/(1.5+0.5)) - (0.75 - 0.25))"), Some(0.5));
        for text in [
            "calc()",
            "calc(5)",
            "calc(b)",
            "calc(5+1-4)",
            "calc(5 + (1.5))",
            "calc(4/0)",
            "cal(4+5)",
            "calcs(4+5)",
            "calc(+5)",
            "calc(*5)",
            "calc(5+)",
            "calc((1+2)",
            "calc(1+2))",
            "calc(calc(1+2)+3)",
            "calc(1e5+1)",
            "calc(q+1)",
        ] {
            assert_eq!(calc(text), None, "{text}");
        }
    }

    #[test]
    fn rejections() {
        for text in [
            "",
            "   ",
            "reddish",
            "#gg0000",
            "hsl(nope)",
            "color(display-p3 1 0 0)",
            "rgb(255 0)",
            "foo(1 2 3)",
            "rgb(1 2 3",
            "#12",
            "#12345",
            "#1234567",
            "#",
            "inf",
            "nan",
            "rgb(inf 0 0)",
            "rgb(NaN 0 0)",
            "rgb(infinity 0 0)",
            "hsl(0 100% 50% 1 1)",
            "rgb(1 2 3,, 0.5)",
            "rgb(1 2 3 / 0.5 / 0.5)",
            "rgb(é 0 0)",
            "rgb(1 2 3 / 0.5é)",
            "rgb(1e 0 0)",
            "rgb(. 0 0)",
            "rgb(1%% 0 0)",
            "hsl(1foo 0% 0%)",
            "ab",
            "a_really_long_color_name_that_does_not_exist",
        ] {
            assert!(parse_css_color(text).is_none(), "{text:?}");
        }
        // Non-ASCII outside the parentheses name also fails, and a bare `(`-less non-hex word.
        assert!(parse_css_color("rgbé(1 2 3)").is_none());
        assert!(parse_css_color("é#fff").is_none());
    }

    #[test]
    fn lexical_rules() {
        assert_same("RGB(255 0 0)", "#ff0000");
        assert_same(" rgb (255 0 0) ", "#ff0000");
        assert_same("RED", "#ff0000");
        assert_same("ReBeCcAPuRpLe", "#663399");
        assert_same("HSL(0DEG 100% 50%)", "#ff0000");
        assert_same("hsl(0.5turn 100% 50%)", "hsl(180 100% 50%)");
        assert_same("hsl(200grad 100% 50%)", "hsl(180 100% 50%)");
        assert_same("hsl(3.14159265rad 100% 50%)", "hsl(180 100% 50%)");
        assert_same("hsl(120, 50, 50)", "#ffffff");
        assert_same("TRANSPARENT", "#00000000");
        assert_same("\u{a0}\u{2003}#f00\u{a0}", "#ff0000");
        assert_same("rgb(.5e2 +5. 1E1)", "rgb(50 5 10)");
        assert_same("hsl(-120 100% 50%)", "hsl(240 100% 50%)");
        assert_same("hsl(480 100% 50%)", "hsl(120 100% 50%)");
        assert_same("rgb(  1 ,2  ,  3  )", "rgb(1 2 3)");
        assert_same("rgb(100% 0% 0%)", "#ff0000");
        assert_same("hsl(0 100% 50% / none)", "#ff000000");
        assert_same("#F00", "#ff0000");
        assert_same("f00", "#ff0000");
        assert_same("f008", "#ff000088");
        assert_same("ff000080", "#ff000080");
    }

    #[test]
    fn gamut_clipping() {
        let clipped = color("oklch(90% 0.4 40)");
        assert!(clipped.iter().all(|c| (0. ..=1.).contains(c)));
        assert_same("oklch(90% 0.4 40)", "#ff0000");
        assert_same("lab(60% 40 50)", "#e17138");
        assert_same("rgb(300 -20 0)", "#ff0000");
        assert_same("oklab(2 0 0)", "#ffffff");
        assert_same("oklab(-1 0 0)", "#000000");
        assert_same("lab(-50 0 0)", "#000000");
        assert_same("oklch(0.5 -1 40)", "oklch(0.5 0 40)");
        // Lab and OKLab lightness percentages and chroma mappings.
        assert_same("lab(50% 0 0)", "lab(50 0 0)");
        assert_same("oklab(62% 0 0)", "oklab(0.62 0 0)");
        assert_same("oklab(50% 100% 0%)", "oklab(0.5 0.4 0)");
        assert_same("lab(50% -100% 100%)", "lab(50 -125 125)");
        assert_same("lch(50% 100% 30deg)", "lch(50 150 30)");
        assert_same("oklch(50% 100% 30)", "oklch(0.5 0.4 30)");
    }

    #[test]
    fn helper_agreement() {
        let text = "oklch(0.62796 0.25768 29.23388 / 50%)";
        let tuple = parse_css_color(text).unwrap();
        let packed = css_color_to_packed(text).unwrap();
        let rgba = css_color_to_rgba(text).unwrap();
        assert_eq!(tuple[3], 0.5);
        assert_eq!(packed & 0xff, 127);
        assert_eq!([rgba.r, rgba.g, rgba.b, rgba.a], tuple);
        assert_eq!(packed, u32::from(rgba));
        assert_eq!(packed >> 24, (tuple[0] * 255.) as u32);
        assert_eq!(css_color_to_packed("#ff0000"), Some(0xff0000ff));
        assert_eq!(css_color_to_packed("nope"), None);
        assert!(css_color_to_rgba("nope").is_none());
    }

    #[test]
    fn hex_agrees_with_rgba_paint_type() {
        for text in [
            "#f09",
            "#f09f",
            "#ff0099",
            "#ff0099ff",
            " #DeAdbEeF ",
            "#0f08",
        ] {
            let expected = Rgba::try_from(text).unwrap();
            assert_eq!(css_color_to_rgba(text), Some(expected), "{text}");
        }
        assert_eq!(Rgba::try_from("#ff0099").unwrap(), crate::rgba(0xff0099ff));
        assert_eq!(
            Rgba::try_from("rgb(255 0 153)").unwrap(),
            crate::rgba(0xff0099ff)
        );
        assert!(Rgba::try_from("nope").is_err());
    }

    #[test]
    fn deterministic() {
        let text = "oklab(from lch(from #bad455 l c h) calc(l * 0.7) a b)";
        assert_eq!(parse_css_color(text), parse_css_color(text));
    }

    #[test]
    fn nesting_is_bounded() {
        let mut text = String::from("#bad455");
        for _ in 0..(MAX_NESTING + 8) {
            text = format!("rgb(from {text} r g b)");
        }
        assert!(parse_css_color(&text).is_none());
        let mut text = String::from("#bad455");
        for _ in 0..8 {
            text = format!("rgb(from {text} r g b)");
        }
        assert_same(&text, "#bad455");
    }

    #[test]
    fn oklch_authoring_helper() {
        let accent = oklch_to_rgba(0.673, 0.182, 276.935, 1.);
        let [r, g, b, _] = [accent.r, accent.g, accent.b, accent.a].map(|c| (c * 255.).round());
        assert_eq!([r, g, b], [124., 134., 255.]);
    }
}
