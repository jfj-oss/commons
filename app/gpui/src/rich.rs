//! A message's words as the web shows them (app/ui/app.js `fmt`, `linkify`): web addresses are links, `**bold**`, `*italic*` and `` `code` ``
//! (inline only, as the page has it) lose their marks and are styled, and `@name` of a person in the room is picked out (yours is filled).
//! The result is the text as it is DRAWN (marks removed) with styled spans over it; selection and copying work on that text, as a browser's
//! selection copies what is shown. Nothing here draws: `runs` makes the text runs for GPUI.
use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind { Bold, Italic, Code, Link(String), At(bool) }

#[derive(Clone, Debug, PartialEq)]
pub struct Span { pub r: Range<usize>, pub k: Kind }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed { pub text: String, pub spans: Vec<Span> }

fn word(c: char) -> bool { c.is_alphanumeric() || c == '_' }

/// Trailing punctuation is not part of an address, unless the address opened that bracket (app.js `linkify`).
fn trim_url(mut url: &str) -> &str {
    while let Some(c) = url.chars().last() {
        if !".,;:!?)]".contains(c) { break; }
        if c == ')' && url.matches('(').count() >= url.matches(')').count() { break; }
        url = &url[..url.len() - c.len_utf8()];
    }
    url
}

/// The display text and the spans of one stretch of ordinary text (no inline code in it), appended to `out`.
fn plain_part(seg: &str, out: &mut Parsed) {
    let b: Vec<(usize, char)> = seg.char_indices().collect();
    let mut i = 0;
    let mut bold: Vec<Range<usize>> = vec![]; // byte ranges in `seg` that are bold (content only)
    // links first (they keep their marks inside), then bold, then italic, like the page's chain of replaces
    let mut links: Vec<(Range<usize>, String)> = vec![];
    while i < b.len() {
        let rest = &seg[b[i].0..];
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let end = rest.char_indices().find(|(_, c)| c.is_whitespace() || "<>\"'".contains(*c)).map(|(k, _)| k).unwrap_or(rest.len());
            let url = trim_url(&rest[..end]);
            let scheme = if rest.starts_with("https://") { 8 } else { 7 };
            if url.len() > scheme { links.push((b[i].0..b[i].0 + url.len(), url.to_string())); i += url.chars().count().max(1); continue; }
        }
        i += 1;
    }
    let in_link = |p: usize| links.iter().any(|(r, _)| r.contains(&p));
    // bold: ** then one or more of anything but * and a newline, then **
    let mut p = 0;
    while let Some(k) = seg[p..].find("**") {
        let s = p + k;
        if in_link(s) { p = s + 2; continue; }
        let body = &seg[s + 2..];
        let n = body.find(|c| c == '*' || c == '\n').unwrap_or(body.len());
        if n > 0 && body[n..].starts_with("**") && !in_link(s + 2) { bold.push(s..s + 2 + n + 2); p = s + 2 + n + 2; } else { p = s + 2; }
    }
    // italic: * not touching a space inside, after the start of a line, a space or "(", before the end or a space or one of .,;:!?)
    let mut italic: Vec<Range<usize>> = vec![];
    let mut p = 0;
    while p < seg.len() {
        let Some(k) = seg[p..].find('*') else { break };
        let s = p + k;
        let covered = bold.iter().any(|r| r.contains(&s)) || in_link(s);
        let before_ok = seg[..s].chars().last().map_or(true, |c| c.is_whitespace() || c == '(');
        if covered || !before_ok { p = s + 1; continue; }
        let body = &seg[s + 1..];
        let Some(close) = body.find(|c| c == '*' || c == '\n').filter(|&n| body[n..].starts_with('*')) else { p = s + 1; continue };
        let inner = &body[..close];
        let (f, l) = (inner.chars().next(), inner.chars().last());
        let after_ok = body[close + 1..].chars().next().map_or(true, |c| c.is_whitespace() || ".,;:!?)".contains(c));
        if f.is_some_and(|c| c != '*' && !c.is_whitespace()) && l.is_some_and(|c| !c.is_whitespace()) && after_ok { italic.push(s..s + 1 + close + 1); p = s + 1 + close + 1; } else { p = s + 1; }
    }
    // write the text out without the marks, remembering where each piece landed
    let base = out.text.len();
    let mut map = vec![0usize; seg.len() + 1];
    let mut o = base;
    let mut gone = vec![false; seg.len() + 1];
    for r in &bold { for k in r.start..r.start + 2 { gone[k] = true; } for k in r.end - 2..r.end { gone[k] = true; } }
    for r in &italic { gone[r.start] = true; gone[r.end - 1] = true; }
    let marks = |p: usize| gone[p];
    for (idx, c) in seg.char_indices() {
        map[idx] = o;
        if !marks(idx) { out.text.push(c); o += c.len_utf8(); }
    }
    map[seg.len()] = o;
    let shift = |r: &Range<usize>, drop_front: usize, drop_back: usize| map[r.start + drop_front]..map[r.end - drop_back];
    for r in &bold { let m = shift(r, 2, 2); if m.start < m.end { out.spans.push(Span { r: m, k: Kind::Bold }); } }
    for r in &italic { let m = shift(r, 1, 1); if m.start < m.end { out.spans.push(Span { r: m, k: Kind::Italic }); } }
    for (r, u) in &links { out.spans.push(Span { r: shift(r, 0, 0), k: Kind::Link(u.clone()) }); }
}

/// Take a message's raw text apart (app.js `fmt` without the people's names; see `mark_names`).
pub fn parse(text: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut rest = text;
    loop {
        // the next `code`: a backtick, one or more characters that are not a backtick or a newline, a backtick
        let mut found = None;
        let mut from = 0;
        while let Some(k) = rest[from..].find('`') {
            let s = from + k;
            let body = &rest[s + 1..];
            let n = body.find(|c| c == '`' || c == '\n').unwrap_or(body.len());
            if n > 0 && body[n..].starts_with('`') { found = Some((s, n)); break; }
            from = s + 1;
        }
        match found {
            Some((s, n)) => {
                plain_part(&rest[..s], &mut out);
                let a = out.text.len();
                out.text.push_str(&rest[s + 1..s + 1 + n]);
                out.spans.push(Span { r: a..out.text.len(), k: Kind::Code });
                rest = &rest[s + 1 + n + 1..];
            }
            None => { plain_part(rest, &mut out); break; }
        }
    }
    out
}

/// The people's names in the room, and mine: `@Name` and `@FirstName` of each (app.js `AT`, `nameAlts`). Mine is filled.
pub fn mark_names(p: &mut Parsed, names: &[String], me: &str) {
    let mut alts: Vec<String> = names.iter().chain(std::iter::once(&me.to_string())).flat_map(|n| { let n = n.trim().to_string(); let f = n.split_whitespace().next().unwrap_or("").to_string(); [n, f] }).filter(|n| !n.is_empty()).collect();
    alts.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then(a.cmp(b))); alts.dedup();
    let mine: Vec<String> = [me.trim().to_string(), me.split_whitespace().next().unwrap_or("").to_string()].into_iter().filter(|n| !n.is_empty()).collect();
    if alts.is_empty() || !p.text.contains('@') { return; }
    let text = p.text.clone();
    let skip = |at: usize| p.spans.iter().any(|s| matches!(s.k, Kind::Code | Kind::Link(_)) && s.r.contains(&at));
    let mut found = vec![];
    for (i, c) in text.char_indices() {
        if c != '@' || skip(i) { continue; }
        if text[..i].chars().last().is_some_and(|b| word(b) || b == '@') { continue; }
        let after = &text[i + 1..];
        for a in &alts {
            let take: String = after.chars().take(a.chars().count()).collect();
            if take.to_lowercase() == a.to_lowercase() && take.len() == a.len().min(take.len()) {
                if after[take.len()..].chars().next().is_some_and(word) { continue; }
                let me_ = mine.iter().any(|m| m.to_lowercase() == take.to_lowercase());
                found.push(Span { r: i..i + 1 + take.len(), k: Kind::At(me_) });
                break;
            }
        }
    }
    p.spans.extend(found);
}

/// The web addresses of a parsed message (for clicks).
pub fn links(p: &Parsed) -> Vec<(Range<usize>, String)> {
    p.spans.iter().filter_map(|s| if let Kind::Link(u) = &s.k { Some((s.r.clone(), u.clone())) } else { None }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn kinds(p: &Parsed) -> Vec<(String, Kind)> { p.spans.iter().map(|s| (p.text[s.r.clone()].to_string(), s.k.clone())).collect() }

    #[test]
    fn plain_text_is_untouched() {
        let p = parse("hello there, 2 * 3 is six");
        assert_eq!(p.text, "hello there, 2 * 3 is six"); assert!(p.spans.is_empty());
    }
    #[test]
    fn bold_italic_and_code_lose_their_marks() {
        let p = parse("a **bold** and *slanted* and `code` end");
        assert_eq!(p.text, "a bold and slanted and code end");
        assert_eq!(kinds(&p), vec![("bold".into(), Kind::Bold), ("slanted".into(), Kind::Italic), ("code".into(), Kind::Code)]);
    }
    #[test]
    fn code_keeps_its_stars() {
        let p = parse("run `a **b** *c*` now");
        assert_eq!(p.text, "run a **b** *c* now"); assert_eq!(kinds(&p), vec![("a **b** *c*".into(), Kind::Code)]);
    }
    #[test]
    fn an_unfinished_mark_stays_as_typed() {
        assert_eq!(parse("**not closed").text, "**not closed");
        assert_eq!(parse("one ` tick").text, "one ` tick");
        assert_eq!(parse("a*b*c").text, "a*b*c");
        assert_eq!(parse("2 * 3 * 4").text, "2 * 3 * 4");
    }
    #[test]
    fn a_link_loses_trailing_punctuation_but_keeps_its_own_bracket() {
        let p = parse("see https://example.com/a_(b)). and (https://x.org/y).");
        assert_eq!(links(&p).iter().map(|l| l.1.as_str()).collect::<Vec<_>>(), vec!["https://example.com/a_(b)", "https://x.org/y"]);
        assert_eq!(p.text, "see https://example.com/a_(b)). and (https://x.org/y).");
    }
    #[test]
    fn a_link_is_not_styled_inside() {
        let p = parse("**https://a.b/c** and *https://d.e*");
        assert_eq!(links(&p).len(), 2);
    }
    #[test]
    fn mentions_pick_the_name_and_the_first_name() {
        let mut p = parse("hi @Bob Smith and @bob, not a@bob or @Bobby");
        mark_names(&mut p, &["Bob Smith".into(), "Carol".into()], "Alice");
        assert_eq!(kinds(&p), vec![("@Bob Smith".into(), Kind::At(false)), ("@bob".into(), Kind::At(false))]);
    }
    #[test]
    fn my_mention_is_marked() {
        let mut p = parse("@alice look");
        mark_names(&mut p, &[], "Alice Jones");
        assert_eq!(kinds(&p), vec![("@alice".into(), Kind::At(true))]);
    }
    #[test]
    fn a_mention_in_code_is_left_alone() {
        let mut p = parse("`@Bob` and @Bob");
        mark_names(&mut p, &["Bob".into()], "Alice");
        assert_eq!(p.spans.iter().filter(|s| matches!(s.k, Kind::At(_))).count(), 1);
    }
    #[test]
    fn multibyte_text_keeps_its_ranges() {
        let p = parse("héllo **wörld** ✓ `ç`");
        assert_eq!(p.text, "héllo wörld ✓ ç");
        assert_eq!(kinds(&p), vec![("wörld".into(), Kind::Bold), ("ç".into(), Kind::Code)]);
    }
}
