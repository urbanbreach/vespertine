//! Tree-construction feedback for the sanitizer's tokenizer.
//!
//! A conforming HTML parser switches its tokenizer into RCDATA, RAWTEXT, script data or PLAINTEXT
//! after certain start tags and enables CDATA sections inside foreign (SVG/MathML) content. The
//! sanitizer does not build a DOM, so this module tracks only what decides those switches: the
//! stack of namespaces entered through `svg`/`math` and the HTML/MathML integration points
//! (HTML Standard 13.2.6.5, "The rules for parsing tokens in foreign content").
//!
//! A few constructs cannot be resolved without the full stack of open elements: text-mode start
//! tags that the tree builder would ignore inside `<select>` or after `<frameset>`. Following the
//! fail-closed policy of the sanitizer these are rejected as ambiguous instead of guessed.

use crate::html_lexer::{TagSpan, TextMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Namespace {
    Html,
    Svg,
    MathMl,
}

/// Input whose parsing context depends on tree-construction state we do not track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AmbiguousMarkup;

/// Changes the driver applies to the tokenizer after a tag.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Feedback {
    pub(crate) text_mode: Option<TextMode>,
    pub(crate) allow_cdata: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Guard {
    Default,
    InSelect,
    InTemplateInSelect(usize),
    InOrAfterFrameset,
}

pub(crate) struct Simulator {
    ns_stack: Vec<Namespace>,
    guard: Guard,
}

fn is_text_mode_tag(name: &str) -> bool {
    matches!(
        name,
        "textarea"
            | "title"
            | "plaintext"
            | "script"
            | "style"
            | "iframe"
            | "xmp"
            | "noembed"
            | "noframes"
            | "noscript"
    )
}

fn text_mode_for(name: &str) -> Option<TextMode> {
    match name {
        "textarea" | "title" => Some(TextMode::RcData),
        "plaintext" => Some(TextMode::PlainText),
        "script" => Some(TextMode::ScriptData),
        "style" | "iframe" | "xmp" | "noembed" | "noframes" | "noscript" => Some(TextMode::RawText),
        _ => None,
    }
}

fn breaks_out_of_foreign_content(name: &str) -> bool {
    matches!(
        name,
        "b" | "big"
            | "blockquote"
            | "body"
            | "br"
            | "center"
            | "code"
            | "dd"
            | "div"
            | "dl"
            | "dt"
            | "em"
            | "embed"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "head"
            | "hr"
            | "i"
            | "img"
            | "li"
            | "listing"
            | "menu"
            | "meta"
            | "nobr"
            | "ol"
            | "p"
            | "pre"
            | "ruby"
            | "s"
            | "small"
            | "span"
            | "strong"
            | "strike"
            | "sub"
            | "sup"
            | "table"
            | "tt"
            | "u"
            | "ul"
            | "var"
    )
}

fn is_mathml_text_integration_point(name: &str) -> bool {
    matches!(name, "mi" | "mo" | "mn" | "ms" | "mtext")
}

fn is_svg_html_integration_point(name: &str) -> bool {
    matches!(name, "desc" | "title" | "foreignobject")
}

impl Simulator {
    pub(crate) fn new() -> Self {
        Self {
            ns_stack: vec![Namespace::Html],
            guard: Guard::Default,
        }
    }

    pub(crate) fn namespace(&self) -> Namespace {
        *self.ns_stack.last().unwrap_or(&Namespace::Html)
    }

    fn cdata_feedback(&self) -> Feedback {
        Feedback {
            text_mode: None,
            allow_cdata: Some(self.namespace() != Namespace::Html),
        }
    }

    fn enter(&mut self, namespace: Namespace) -> Feedback {
        self.ns_stack.push(namespace);
        self.cdata_feedback()
    }

    /// Pops foreign namespaces until HTML content (or an integration point) is current again.
    fn leave_foreign(&mut self) -> Feedback {
        while self.ns_stack.len() > 1 && self.namespace() != Namespace::Html {
            self.ns_stack.pop();
        }
        self.cdata_feedback()
    }

    /// Leaves exactly the current namespace.
    fn leave_one(&mut self) -> Feedback {
        if self.ns_stack.len() > 1 {
            self.ns_stack.pop();
        }
        self.cdata_feedback()
    }

    fn track_start_for_ambiguity(&mut self, name: &str) -> Result<(), AmbiguousMarkup> {
        let reject_text_mode = |name: &str| {
            if is_text_mode_tag(name) {
                Err(AmbiguousMarkup)
            } else {
                Ok(())
            }
        };
        match self.guard {
            Guard::Default => match name {
                "select" => self.guard = Guard::InSelect,
                "frameset" => self.guard = Guard::InOrAfterFrameset,
                _ => {}
            },
            Guard::InSelect => match name {
                "select" | "textarea" | "input" | "keygen" => self.guard = Guard::Default,
                "template" => self.guard = Guard::InTemplateInSelect(1),
                // `<script>` is allowed in the "in select" insertion mode.
                "script" => {}
                _ => reject_text_mode(name)?,
            },
            Guard::InTemplateInSelect(depth) => {
                if name == "template" {
                    self.guard = Guard::InTemplateInSelect(depth + 1);
                } else {
                    reject_text_mode(name)?;
                }
            }
            Guard::InOrAfterFrameset => {
                // `<noframes>` is allowed in and after `<frameset>`.
                if name != "noframes" {
                    reject_text_mode(name)?;
                }
            }
        }
        Ok(())
    }

    fn track_end_for_ambiguity(&mut self, name: &str) {
        match self.guard {
            Guard::InSelect if name == "select" => self.guard = Guard::Default,
            Guard::InTemplateInSelect(depth) if name == "template" => {
                self.guard = if depth == 1 {
                    Guard::InSelect
                } else {
                    Guard::InTemplateInSelect(depth - 1)
                };
            }
            _ => {}
        }
    }

    /// Feedback for a start tag. `name` is the lowercase tag name.
    pub(crate) fn start_tag(
        &mut self,
        name: &str,
        tag: &TagSpan,
        source: &str,
    ) -> Result<Feedback, AmbiguousMarkup> {
        self.track_start_for_ambiguity(name)?;

        if name == "svg" || name == "math" {
            // A self-closing foreign element is popped again right away.
            if tag.self_closing {
                return Ok(Feedback::default());
            }
            return Ok(self.enter(if name == "svg" {
                Namespace::Svg
            } else {
                Namespace::MathMl
            }));
        }

        let namespace = self.namespace();
        if namespace == Namespace::Html {
            return Ok(Feedback {
                text_mode: text_mode_for(name),
                allow_cdata: None,
            });
        }

        if breaks_out_of_foreign_content(name) {
            return Ok(self.leave_foreign());
        }
        if (namespace == Namespace::Svg && is_svg_html_integration_point(name))
            || (namespace == Namespace::MathMl && is_mathml_text_integration_point(name))
        {
            if tag.self_closing {
                return Ok(Feedback::default());
            }
            return Ok(self.enter(Namespace::Html));
        }
        if name == "font" {
            let breaks_out = tag.attrs.iter().any(|attr| {
                let attr_name = &source[attr.name.clone()];
                attr_name.eq_ignore_ascii_case("color")
                    || attr_name.eq_ignore_ascii_case("size")
                    || attr_name.eq_ignore_ascii_case("face")
            });
            if breaks_out {
                return Ok(self.leave_foreign());
            }
        }
        if name == "annotation-xml" && namespace == Namespace::MathMl && !tag.self_closing {
            let html_encoding = tag.attrs.iter().any(|attr| {
                source[attr.name.clone()].eq_ignore_ascii_case("encoding") && {
                    let value = &source[attr.value.clone()];
                    value.eq_ignore_ascii_case("text/html")
                        || value.eq_ignore_ascii_case("application/xhtml+xml")
                }
            });
            if html_encoding {
                return Ok(self.enter(Namespace::Html));
            }
        }
        Ok(Feedback::default())
    }

    /// Feedback for an end tag. `name` is the lowercase tag name.
    pub(crate) fn end_tag(&mut self, name: &str) -> Feedback {
        self.track_end_for_ambiguity(name);

        match self.namespace() {
            Namespace::Html => {
                if self.ns_stack.len() < 2 {
                    return Feedback::default();
                }
                let parent = self.ns_stack[self.ns_stack.len() - 2];
                let leaves = match parent {
                    Namespace::MathMl => {
                        is_mathml_text_integration_point(name) || name == "annotation-xml"
                    }
                    Namespace::Svg => is_svg_html_integration_point(name),
                    Namespace::Html => false,
                };
                if leaves {
                    self.leave_one()
                } else {
                    Feedback::default()
                }
            }
            Namespace::Svg if name == "svg" => self.leave_one(),
            Namespace::MathMl if name == "math" => self.leave_one(),
            _ if name == "p" || name == "br" => self.leave_foreign(),
            _ => Feedback::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_lexer::{Lexer, Token};

    fn start(src: &str) -> (TagSpan, String) {
        let mut lexer = Lexer::new(src);
        match lexer.next_token() {
            Some(Token::StartTag(tag)) => (tag, src.to_string()),
            other => panic!("expected start tag, got {other:?}"),
        }
    }

    #[test]
    fn text_mode_start_tags_switch_only_in_html_content() {
        let mut sim = Simulator::new();
        let (tag, src) = start("<textarea>");
        assert_eq!(
            sim.start_tag("textarea", &tag, &src).unwrap().text_mode,
            Some(TextMode::RcData)
        );

        let (svg, src) = start("<svg>");
        assert_eq!(
            sim.start_tag("svg", &svg, &src).unwrap().allow_cdata,
            Some(true)
        );
        let (tag, src) = start("<style>");
        assert_eq!(sim.start_tag("style", &tag, &src).unwrap().text_mode, None);
        assert_eq!(sim.end_tag("svg").allow_cdata, Some(false));
        assert_eq!(sim.namespace(), Namespace::Html);
    }

    #[test]
    fn html_integration_points_and_breakout_tags_restore_html_content() {
        let mut sim = Simulator::new();
        let (tag, src) = start("<svg>");
        sim.start_tag("svg", &tag, &src).unwrap();
        let (tag, src) = start("<foreignObject>");
        sim.start_tag("foreignobject", &tag, &src).unwrap();
        assert_eq!(sim.namespace(), Namespace::Html);
        let (tag, src) = start("<script>");
        assert_eq!(
            sim.start_tag("script", &tag, &src).unwrap().text_mode,
            Some(TextMode::ScriptData)
        );
        sim.end_tag("foreignobject");
        assert_eq!(sim.namespace(), Namespace::Svg);
        let (tag, src) = start("<div>");
        sim.start_tag("div", &tag, &src).unwrap();
        assert_eq!(sim.namespace(), Namespace::Html);

        let mut sim = Simulator::new();
        let (tag, src) = start("<math>");
        sim.start_tag("math", &tag, &src).unwrap();
        sim.end_tag("p");
        assert_eq!(sim.namespace(), Namespace::Html);
    }

    #[test]
    fn text_mode_tags_in_select_and_frameset_are_ambiguous() {
        for (open, tag) in [
            ("<select>", "xmp"),
            ("<select>", "style"),
            ("<select>", "title"),
            ("<select><template>", "script"),
            ("<frameset>", "script"),
        ] {
            let mut sim = Simulator::new();
            for part in open.split_inclusive('>') {
                let (t, src) = start(part);
                let name = part.trim_matches(|c| c == '<' || c == '>');
                sim.start_tag(name, &t, &src).unwrap();
            }
            let markup = format!("<{tag}>");
            let (t, src) = start(&markup);
            assert_eq!(sim.start_tag(tag, &t, &src), Err(AmbiguousMarkup), "{open}{tag}");
        }

        let mut sim = Simulator::new();
        let (t, src) = start("<select>");
        sim.start_tag("select", &t, &src).unwrap();
        let (t, src) = start("<script>");
        assert!(sim.start_tag("script", &t, &src).is_ok());
        sim.end_tag("select");
        let (t, src) = start("<xmp>");
        assert!(sim.start_tag("xmp", &t, &src).is_ok());

        let mut sim = Simulator::new();
        let (t, src) = start("<frameset>");
        sim.start_tag("frameset", &t, &src).unwrap();
        let (t, src) = start("<noframes>");
        assert!(sim.start_tag("noframes", &t, &src).is_ok());
    }
}
