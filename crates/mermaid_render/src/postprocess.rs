//! Zed-specific post-processing of [`merman`]-produced SVGs.
//!
//! Each submodule is a specific pass that tweaks the SVG event iterator in a particular way.
//!
//! We always produce and consume [`Event`]s with a short lifetime.
//! [`Event<'a>`] is backed internally by a [`Cow<'a, [u8]>`](std::borrow::Cow),
//! so we don't have lifetime issues when we need to mutate the text in an
//! [`Event`], but also don't force allocating a new [`String`] each time.
//!
//! Many modules contain internal structs that implement [`Iterator`] to make
//! reasoning about lifetimes simpler, but these are private implementation
//! details.

mod accent_colors;
mod element_fixup;
mod inject_css;
mod strip_foreignobject;
pub(crate) mod util;

use anyhow::{Context as _, Result};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::MermaidTheme;

pub(super) fn postprocess(svg: &str, theme: &MermaidTheme) -> Result<String> {
    postprocess_with_palette(svg, Some(theme))
}

pub(super) fn postprocess_with_palette(svg: &str, theme: Option<&MermaidTheme>) -> Result<String> {
    // merman 0.6 already applies the generic resvg-safe cleanup before this point.
    // The remaining passes are Zed-specific theme and accent adjustments.
    let svg_id = extract_svg_id(svg);

    let mut reader = Reader::from_str(svg);
    reader.config_mut().check_end_names = false;
    let events = ReaderIter::new(reader);
    // merman's resvg-safe pipeline already removes foreignObject elements and
    // replaces their labels with native <text> fallback groups. This pass keeps
    // those fallback labels, but drops any that merely duplicate a native
    // <text> (e.g. user journey renders some labels both ways).
    let events = strip_foreignobject::process(events, svg);
    let events = events.map(|event| preserve_fallback_text_color(event?));
    let events: Box<dyn Iterator<Item = Result<Event<'_>>>> = if let Some(theme) = theme {
        let events = element_fixup::process(events, theme);
        let events = accent_colors::process(events, theme);
        Box::new(inject_css::process(events, theme, &svg_id))
    } else {
        Box::new(events)
    };

    let mut writer = quick_xml::Writer::new(Vec::with_capacity(svg.len()));
    for event in events {
        writer.write_event(event?)?;
    }
    String::from_utf8(writer.into_inner()).context("SVG output is not valid UTF-8")
}

fn preserve_fallback_text_color(event: Event<'_>) -> Result<Event<'_>> {
    if let Event::Start(element) | Event::Empty(element) = &event
        && element.name().as_ref() == b"g"
        && accent_colors::is_foreign_object_fallback_group(element)?
    {
        let mut replacement = BytesStart::new("g");
        for attribute in element.attributes() {
            let attribute = attribute?;
            if attribute.key.as_ref() != b"class" {
                replacement.push_attribute(attribute);
            }
        }
        // Copied node classes would apply shape fill rules to the moved text.
        replacement.push_attribute(("class", "merman-foreignobject-fallback"));
        return Ok(match event {
            Event::Start(_) => Event::Start(replacement.into_owned()),
            _ => Event::Empty(replacement.into_owned()),
        });
    }
    let element = match &event {
        Event::Start(element) | Event::Empty(element) if element.name().as_ref() == b"text" => {
            element
        }
        _ => return Ok(event),
    };
    let is_fallback = element
        .try_get_attribute("class")?
        .map(|class| {
            class.unescape_value().map(|class| {
                class
                    .split_whitespace()
                    .any(|class| class == "merman-foreignobject-fallback-text")
            })
        })
        .transpose()?
        .unwrap_or(false);
    if !is_fallback {
        return Ok(event);
    }
    let Some(fill) = element.try_get_attribute("fill")? else {
        return Ok(event);
    };
    let fill = fill.unescape_value()?;
    let mut style = element
        .try_get_attribute("style")?
        .map(|style| style.unescape_value().map(|style| style.into_owned()))
        .transpose()?
        .unwrap_or_default();
    // Merman resolves HTML label colors before moving fallback text out of
    // the node. Keep that computed color above CSS for the new ancestry.
    style.push_str(&format!(";fill:{fill} !important;"));
    let mut replacement = BytesStart::new("text");
    for attribute in element.attributes() {
        let attribute = attribute?;
        if !matches!(attribute.key.as_ref(), b"style" | b"class") {
            replacement.push_attribute(attribute);
        }
    }
    replacement.push_attribute(("class", "merman-foreignobject-fallback-text"));
    replacement.push_attribute(("style", style.as_str()));
    Ok(match event {
        Event::Start(_) => Event::Start(replacement.into_owned()),
        _ => Event::Empty(replacement.into_owned()),
    })
}

fn extract_svg_id(svg: &str) -> String {
    let mut reader = Reader::from_str(svg);
    reader.config_mut().check_end_names = false;
    for event in ReaderIter::new(reader) {
        let Ok(Event::Start(e) | Event::Empty(e)) = event else {
            continue;
        };
        if e.name().as_ref() == b"svg" {
            return e
                .try_get_attribute("id")
                .ok()
                .flatten()
                .and_then(|a| a.unescape_value().ok())
                .map(|v| v.into_owned())
                .unwrap_or_default();
        }
    }
    String::new()
}

struct ReaderIter<'a> {
    reader: Reader<&'a [u8]>,
    done: bool,
}

impl<'a> ReaderIter<'a> {
    fn new(reader: Reader<&'a [u8]>) -> Self {
        Self {
            reader,
            done: false,
        }
    }
}

impl<'a> Iterator for ReaderIter<'a> {
    type Item = Result<Event<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.reader.read_event() {
            Ok(Event::Eof) => {
                self.done = true;
                None
            }
            Ok(event) => Some(Ok(event)),
            Err(e) => {
                self.done = true;
                Some(Err(e.into()))
            }
        }
    }
}
