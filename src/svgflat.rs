//! Turns Pele's themeable SVG into plain SVG that resvg can draw.
//!
//! Pele styles everything through CSS custom properties and the occasional
//! `color-mix()`. resvg understands neither, so we substitute the values: our
//! theme for `--pele-*`, the root element's own declarations for Pele's
//! internal `--_*` aliases, and the `var()` fallback for anything else.

use crate::theme::{Rgb, Theme};
use std::collections::HashMap;

pub fn flatten(svg: &str, theme: &Theme) -> String {
    let decls = root_declarations(svg);
    strip_custom_props(&eval_value(svg, &decls, theme, 0))
}

/// Index one past the `)` that closes the first `(` in `s`.
fn matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b'"' | b'<' | b'>' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// Evaluates every `var()` and `color-mix()` inside a CSS value.
fn eval_value(v: &str, decls: &HashMap<String, String>, theme: &Theme, depth: u8) -> String {
    let mut out = String::new();
    let mut rest = v;
    loop {
        let next = [rest.find("var("), rest.find("color-mix(")].into_iter().flatten().min();
        let Some(j) = next else { break };
        out.push_str(&rest[..j]);
        let tail = &rest[j..];
        let Some(end) = matching_paren(tail) else {
            out.push_str(tail);
            return out;
        };
        out.push_str(&eval(&tail[..end], decls, theme, depth + 1));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn eval(call: &str, decls: &HashMap<String, String>, theme: &Theme, depth: u8) -> String {
    if depth > 8 {
        return String::new();
    }
    if let Some(inner) = call.strip_prefix("var(").and_then(|s| s.strip_suffix(')')) {
        let (name, fallback) = match split_top(inner, ',') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inner.trim(), None),
        };
        if let Some(v) = theme.pele_var(name) {
            return v;
        }
        if let Some(v) = decls.get(name) {
            return eval_value(v, decls, theme, depth);
        }
        return fallback.map(|f| eval_value(f, decls, theme, depth)).unwrap_or_default();
    }
    if let Some(inner) = call.strip_prefix("color-mix(").and_then(|s| s.strip_suffix(')')) {
        let inner = eval_value(inner, decls, theme, depth);
        return color_mix(&inner).unwrap_or_else(|| first_color(&inner));
    }
    call.to_string()
}

/// `in srgb, A P%, B Q%` with either percentage optional.
fn color_mix(inner: &str) -> Option<String> {
    let parts: Vec<&str> = split_all_top(inner, ',');
    if parts.len() != 3 {
        return None;
    }
    let parse = |s: &str| -> Option<(Rgb, Option<f32>)> {
        let s = s.trim();
        match s.rsplit_once(' ') {
            Some((c, p)) if p.ends_with('%') => Some((Rgb::parse(c)?, p.trim_end_matches('%').parse().ok())),
            _ => Some((Rgb::parse(s)?, None)),
        }
    };
    let (a, pa) = parse(parts[1])?;
    let (b, pb) = parse(parts[2])?;
    let wa = match (pa, pb) {
        (Some(p), _) => p / 100.0,
        (None, Some(q)) => 1.0 - q / 100.0,
        (None, None) => 0.5,
    };
    Some(a.mix(b, 1.0 - wa).hex())
}

fn first_color(s: &str) -> String {
    s.split([',', ' ']).find(|t| t.starts_with('#')).unwrap_or("#888").to_string()
}

fn split_top(s: &str, sep: char) -> Option<(&str, &str)> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => return Some((&s[..i], &s[i + 1..])),
            _ => {}
        }
    }
    None
}

fn split_all_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some((a, b)) = split_top(rest, sep) {
        out.push(a);
        rest = b;
    }
    out.push(rest);
    out
}

/// Custom properties declared in the root `<svg style="...">`.
fn root_declarations(svg: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(tag_end) = svg.find('>') else { return map };
    let tag = &svg[..tag_end];
    let Some(start) = tag.find("style=\"") else { return map };
    let style = &tag[start + 7..];
    let style = &style[..style.find('"').unwrap_or(style.len())];
    for decl in split_all_top(style, ';') {
        if let Some((k, v)) = decl.split_once(':') {
            let k = k.trim();
            if k.starts_with("--") {
                map.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
    map
}

/// Drops `--name:value;` declarations, which resvg would only skip anyway.
fn strip_custom_props(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(i) = rest.find("style=\"") {
        out.push_str(&rest[..i + 7]);
        let body = &rest[i + 7..];
        let end = body.find('"').unwrap_or(body.len());
        let kept: Vec<&str> = split_all_top(&body[..end], ';')
            .into_iter()
            .filter(|d| !d.trim_start().starts_with("--") && !d.trim().is_empty())
            .collect();
        out.push_str(&kept.join(";"));
        rest = &body[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_aliases_theme_and_fallbacks() {
        let theme = Theme::dark_default();
        let svg = r##"<svg style="--_fg:var(--pele-fg,#1f1f1f);--_x:var(--unknown,#123456);color:var(--_fg)" fill="var(--_x)"><rect rx="var(--_r)"/></svg>"##;
        let out = flatten(svg, &theme);
        assert!(!out.contains("var("), "{out}");
        assert!(out.contains(&format!("color:{}", theme.fg.hex())), "{out}");
        assert!(out.contains(r##"fill="#123456""##), "{out}");
        assert!(!out.contains("--_"), "{out}");
    }

    #[test]
    fn evaluates_color_mix() {
        let theme = Theme::dark_default();
        let svg = r##"<svg style="--_a:#000000;--_bg:#ffffff"><g stroke="color-mix(in srgb,var(--_a) 25%,var(--_bg))"/></svg>"##;
        let out = flatten(svg, &theme);
        assert!(out.contains(r##"stroke="#bfbfbf""##), "{out}");
    }
}
