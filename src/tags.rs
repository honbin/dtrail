//! Shape rules for harness tags in user text, shared by the conversation body
//! (folding, dimming) and the list preview (finding the user's own words) so
//! both read a message the same way. Nothing here knows tag names except
//! Cursor's prompt wrapper.

/// Tag Cursor wraps the typed prompt in, apart from the injected context
/// around it. The preview reads the prompt from inside it, and the body view
/// keeps it unfolded for the same reason.
pub(crate) const USER_QUERY_TAG: &str = "user_query";

/// Lines `start..=end` of a text block, from a line opening `<name ...>` to
/// the line closing it with `</name>`. Either line may also carry text, as in
/// `<a>first` … `last</a>`.
pub(crate) struct Region<'a> {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) name: &'a str,
}

/// Outermost multi-line tag regions in user text. Harness context is often
/// wrapped this way; matching on shape rather than a list of tag names keeps
/// dtrail from tracking each agent's vocabulary. A user's own pasted markup
/// may fold too, which is harmless since folding hides nothing for good.
///
/// A line that closes one region may open the next (`</a><b>`). It stays in
/// the region it closes and the next starts below it, so unfolding both never
/// shows a line twice.
///
/// The one exception is Cursor's prompt wrapper, which the preview already
/// knows: folding it would hide what the user typed. Keep it the only one.
pub(crate) fn tagged_regions<'a>(lines: &[&'a str]) -> Vec<Region<'a>> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut carried: Option<(usize, &str)> = None;
    while i < lines.len() {
        let pending = carried.take();
        let (open, start, name) = match pending {
            Some((open, name)) => (open, open + 1, Some(name)),
            None => (i, i, region_open(lines[i])),
        };
        if let Some(name) = name
            && name != USER_QUERY_TAG
            && let Some(end) = matching_close(lines, open, name)
        {
            out.push(Region { start, end, name });
            carried = last_open_tag(lines[end]).map(|n| (end, n));
            i = end + 1;
        } else if pending.is_none() {
            i += 1;
        }
    }
    out
}

/// The opening or closing tag (attributes allowed) at the start of `s`, as
/// `(is_closing, name)`, and the text after it.
fn leading_tag(s: &str) -> Option<((bool, &str), &str)> {
    let (inner, after) = s.strip_prefix('<')?.split_once('>')?;
    let (closing, inner) = match inner.strip_prefix('/') {
        Some(inner) => (true, inner),
        None => (false, inner),
    };
    let (name, attrs) = split_tag_name(inner)?;
    (attrs.is_empty() || attrs.starts_with(char::is_whitespace)).then_some(((closing, name), after))
}

/// The tags of `line` as `(is_closing, name)`, if it is nothing but one or more
/// opening or closing tags separated by whitespace.
fn line_tags(line: &str) -> Option<Vec<(bool, &str)>> {
    let mut rest = line.trim();
    let mut tags = Vec::new();
    while !rest.is_empty() {
        let (tag, after) = leading_tag(rest)?;
        tags.push(tag);
        rest = after.trim_start();
    }
    (!tags.is_empty()).then_some(tags)
}

/// `text` without its tags, whitespace collapsed: a readable form of text that
/// is only scaffolding, such as a command wrapper shown as a preview.
pub(crate) fn strip_tags(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match leading_tag(rest) {
            Some((_, after)) => {
                out.push(' ');
                rest = after;
            }
            None => {
                out.push('<');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The name of a tags-only line's last tag, if that tag opens.
fn last_open_tag(line: &str) -> Option<&str> {
    match line_tags(line)?.last()? {
        (false, name) => Some(name),
        _ => None,
    }
}

/// The tag `line` opens a region with: a tags-only line's last opening tag,
/// or a leading `<name ...>` followed by text the line doesn't close.
fn region_open(line: &str) -> Option<&str> {
    if let Some(name) = last_open_tag(line) {
        return Some(name);
    }
    let ((closing, name), rest) = leading_tag(line.trim())?;
    (!closing && !rest.trim().is_empty() && !rest.contains(&format!("</{name}>"))).then_some(name)
}

/// Index of the line closing the region opened at `open`: a tags-only line
/// whose first tag is `</name>`, or a line of text ending in `</name>`. The
/// region must hold some text, on its own lines or beside its tags; same-name
/// nesting is counted.
fn matching_close(lines: &[&str], open: usize, name: &str) -> Option<usize> {
    let close = format!("</{name}>");
    let open_has_text = line_tags(lines[open]).is_none();
    let mut depth = 0;
    for (j, line) in lines.iter().enumerate().skip(open + 1) {
        let tags = line_tags(line);
        let closes_tags = tags.as_ref().and_then(|t| t.first()) == Some(&(true, name));
        // A whole `<name>…</name>` element on one line closes nothing.
        let closes_text = tags.is_none()
            && line.trim_end().ends_with(&close)
            && !line.contains(&format!("<{name}"));
        if closes_tags || closes_text {
            if depth == 0 {
                let has_text = j > open + 1 || open_has_text || closes_text;
                return has_text.then_some(j);
            }
            depth -= 1;
        } else if region_open(line) == Some(name) {
            depth += 1;
        }
    }
    None
}

/// A line made only of tags and whole `<name ...>...</name>` elements, such as
/// `</a><b>` or command wrappers, caveats and timestamps. The preview joins a
/// message onto one line, so several elements in a row must still count.
/// Such lines are dimmed rather than folded: folding one line saves nothing and
/// would hide which command ran. Any tag name qualifies: a user's own one-line
/// markup matching too costs only a dimmed line. A one-line `<user_query>` holds
/// the prompt, so it stays bright. This is a narrow exception to styling by
/// structure only, not a start of styling by content.
pub(crate) fn tag_line(line: &str) -> bool {
    let mut rest = line.trim();
    if rest.is_empty() {
        return false;
    }
    while !rest.is_empty() {
        let Some(((closing, name), after)) = leading_tag(rest) else {
            return false;
        };
        rest = after;
        if !closing && let Some((_, past)) = after.split_once(&format!("</{name}>")) {
            if name == USER_QUERY_TAG {
                return false;
            }
            rest = past;
        }
        rest = rest.trim_start();
    }
    true
}

/// Split a leading tag name (a letter, then letters, digits, `_` or `-`) from
/// the rest.
fn split_tag_name(s: &str) -> Option<(&str, &str)> {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(s.len());
    let (name, after) = s.split_at(end);
    name.starts_with(|c: char| c.is_ascii_alphabetic())
        .then_some((name, after))
}

/// The user's own words in `text`: what remains once tag regions and
/// tag-only or whole-element lines are set aside, or `None` if nothing does.
/// An opening tag that never closes is taken to wrap the rest, so a harness
/// block cut short doesn't surface as a prompt.
pub(crate) fn prompt_text(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut regions = tagged_regions(&lines).into_iter().peekable();
    let mut kept = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        while regions.next_if(|r| r.end < i).is_some() {}
        if regions.peek().is_some_and(|r| r.start <= i) {
            continue;
        }
        if last_open_tag(line).is_some() {
            break;
        }
        if !tag_line(line) {
            kept.push(*line);
        }
    }
    let kept = kept.join("\n");
    (!kept.trim().is_empty()).then(|| kept.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_tags_fold_as_their_outermost_region() {
        let lines = ["<a>", "<a>", "x", "</a>", "</a>", "<b>", "y", "</b>"];
        let regions = tagged_regions(&lines);
        let spans: Vec<_> = regions.iter().map(|r| (r.start, r.end, r.name)).collect();
        assert_eq!(spans, vec![(0, 4, "a"), (5, 7, "b")]);
    }

    #[test]
    fn only_whole_line_tags_are_dimmed() {
        for (line, dimmed) in [
            ("<command-name>/clear</command-name>", true),
            (
                "<command-message>c</command-message> <command-name>/c</command-name>",
                true,
            ),
            ("<a>x</a> now fix it", false),
            ("<command-args></command-args>", true),
            ("  <local-command-caveat>note</local-command-caveat>", true),
            ("<ide_opened_file kind=\"x\">a.rs</ide_opened_file>", true),
            ("<user_query>fix it</user_query>", false),
            ("<timestamp>Friday</timestamp>", true),
            ("<div>pasted</div>", true),
            ("<user_query>", true),
            ("</user_query>", true),
            ("</INSTRUCTIONS><environment_context>", true),
            ("</a> <b k=\"v\">", true),
            ("</a> text", false),
            ("<a<b>", false),
            ("<a>b", false),
            ("<command-name>/clear</command-name> then more", false),
            ("<command-name>/clear</other-name>", false),
            ("why does <command-name>x</command-name> show", false),
        ] {
            assert_eq!(tag_line(line), dimmed, "{line:?}");
        }
    }

    #[test]
    fn prompt_text_keeps_only_the_users_words() {
        for (text, prompt) in [
            ("<a_b>\npaste\n</a_b>\n\nreview this", Some("review this")),
            ("<t>now</t>\n<q>\nfix it\n</q>", None),
            ("see below\n<c>\nx\n</c>\nthanks", Some("see below\nthanks")),
            ("</a><b>\nctx\n</b>\nhi", Some("hi")),
            (
                "<command-name>/clear</command-name>\n<command-args></command-args>",
                None,
            ),
            ("hi\n<open_only>\nrest", Some("hi")),
            ("<div> isn't working", Some("<div> isn't working")),
        ] {
            assert_eq!(prompt_text(text).as_deref(), prompt, "{text:?}");
        }
    }

    #[test]
    fn a_region_may_open_and_close_beside_text() {
        let lines = ["<args>first", "middle", "last</args>", "<b>x</b>"];
        let spans: Vec<_> = tagged_regions(&lines)
            .iter()
            .map(|r| (r.start, r.end, r.name))
            .collect();
        assert_eq!(spans, vec![(0, 2, "args")]);
        // Two lines suffice when the tags carry the text.
        let spans: Vec<_> = tagged_regions(&["<a>x", "y</a>"])
            .iter()
            .map(|r| (r.start, r.end))
            .collect();
        assert_eq!(spans, vec![(0, 1)]);
        // A one-line element inside doesn't close the region early.
        let spans: Vec<_> = tagged_regions(&["<a>x", "<a>y</a>", "z</a>"])
            .iter()
            .map(|r| (r.start, r.end))
            .collect();
        assert_eq!(spans, vec![(0, 2)]);
    }

    #[test]
    fn strip_tags_keeps_the_text_between_tags() {
        assert_eq!(
            strip_tags(
                "<command-message>check</command-message> <command-name>/check</command-name>"
            ),
            "check /check"
        );
        assert_eq!(
            strip_tags("<a x=\"1\">\n  b </a><c/> 1 < 2"),
            "b <c/> 1 < 2"
        );
    }

    #[test]
    fn command_args_spanning_lines_leave_no_prompt() {
        let text = "<command-name>/clear</command-name>\n    <command-message>clear</command-message>\n    <command-args>long\nargs</command-args>\n<local-command-stdout></local-command-stdout>";
        assert_eq!(prompt_text(text), None);
    }
}
