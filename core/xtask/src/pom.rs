//! Maven POM reading: a small XML element tree (elements, text, entities, CDATA; no schema, no
//! namespaces beyond ignoring prefixes) and the handful of POM fields the licence generator needs.

use crate::{Result, fail};

#[derive(Debug, Default)]
pub struct Element {
    pub name: String,
    /// Text before the first child element.
    pub text: String,
    pub children: Vec<Element>,
}

impl Element {
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The trimmed text of the child `name`, empty when there is none.
    pub fn child_text(&self, name: &str) -> String {
        self.child(name)
            .map(|c| c.text.trim().to_string())
            .unwrap_or_default()
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.find(';').and_then(|end| {
            let entity = &rest[1..end];
            let c = match entity {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                _ => {
                    let number = entity.strip_prefix('#')?;
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => number.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((c, end + 1))
        });
        match decoded {
            Some((c, used)) => {
                out.push(c);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Skips to the end of the markup that starts at `at` (a `<`), honouring quoted attribute values.
fn tag_end(xml: &str, at: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, c) in xml[at..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(at + offset),
            _ => {}
        }
    }
    None
}

fn local_name(tag: &str) -> &str {
    tag.rsplit(':').next().unwrap_or(tag)
}

/// Parses the document element of `xml`.
pub fn parse_xml(xml: &str) -> Result<Element> {
    let malformed = |what: &str| format!("malformed XML: {what}");
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut at = 0;
    while at < xml.len() {
        let Some(open) = xml[at..].find('<').map(|i| at + i) else {
            break;
        };
        if let Some(top) = stack.last_mut()
            && top.children.is_empty()
        {
            top.text.push_str(&unescape(&xml[at..open]));
        }
        let rest = &xml[open..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").ok_or_else(|| malformed("comment"))?;
            at = open + end + 3;
        } else if rest.starts_with("<![CDATA[") {
            let end = rest.find("]]>").ok_or_else(|| malformed("CDATA"))?;
            if let Some(top) = stack.last_mut()
                && top.children.is_empty()
            {
                top.text.push_str(&rest[9..end]);
            }
            at = open + end + 3;
        } else if rest.starts_with("<?") {
            let end = rest
                .find("?>")
                .ok_or_else(|| malformed("processing instruction"))?;
            at = open + end + 2;
        } else if rest.starts_with("<!") {
            // A DOCTYPE; an internal subset in brackets is skipped with its closing `]>`.
            let end = if let Some(bracket) = rest.find('[') {
                let after = rest.find("]>").ok_or_else(|| malformed("DOCTYPE"))?;
                if bracket < after {
                    after + 1
                } else {
                    tag_end(xml, open).ok_or_else(|| malformed("DOCTYPE"))? - open
                }
            } else {
                tag_end(xml, open).ok_or_else(|| malformed("DOCTYPE"))? - open
            };
            at = open + end + 1;
        } else if let Some(closing) = rest.strip_prefix("</") {
            let end = closing.find('>').ok_or_else(|| malformed("end tag"))?;
            let name = local_name(closing[..end].trim());
            let element = stack.pop().ok_or_else(|| malformed("unbalanced end tag"))?;
            if element.name != name {
                return fail(malformed(&format!("</{name}> closes <{}>", element.name)));
            }
            match stack.last_mut() {
                Some(parent) => parent.children.push(element),
                None => root = Some(element),
            }
            at = open + 2 + end + 1;
        } else {
            let end = tag_end(xml, open).ok_or_else(|| malformed("start tag"))?;
            let inner = &xml[open + 1..end];
            let self_closing = inner.ends_with('/');
            let inner = inner.trim_end_matches('/');
            let name = local_name(inner.split_whitespace().next().unwrap_or(""));
            if name.is_empty() {
                return fail(malformed("empty tag name"));
            }
            let element = Element {
                name: name.to_string(),
                ..Element::default()
            };
            if self_closing {
                match stack.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => root = Some(element),
                }
            } else {
                stack.push(element);
            }
            at = end + 1;
        }
    }
    if !stack.is_empty() {
        return fail(malformed("unclosed element"));
    }
    root.ok_or_else(|| malformed("no document element"))
}

#[derive(Debug, PartialEq, Eq)]
pub struct PomLicense {
    pub name: String,
    pub url: String,
}

/// What a POM says about itself. `licenses` is empty when the POM lists none; the caller walks
/// `parent` for inherited ones.
#[derive(Debug)]
pub struct Pom {
    pub name: String,
    pub url: String,
    pub licenses: Vec<PomLicense>,
    pub packaging: String,
    /// `(groupId, artifactId, version)` of the parent POM.
    pub parent: Option<(String, String, String)>,
}

pub fn parse_pom(xml: &str) -> Result<Pom> {
    let root = parse_xml(xml)?;
    let licenses = root
        .child("licenses")
        .map(|licenses| {
            licenses
                .children_named("license")
                .map(|l| PomLicense {
                    name: l.child_text("name"),
                    url: l.child_text("url"),
                })
                .collect()
        })
        .unwrap_or_default();
    let parent = root.child("parent").map(|p| {
        (
            p.child_text("groupId"),
            p.child_text("artifactId"),
            p.child_text("version"),
        )
    });
    let packaging = match root.child_text("packaging") {
        p if p.is_empty() => "jar".to_string(),
        p => p,
    };
    Ok(Pom {
        name: root.child_text("name"),
        url: root.child_text("url"),
        licenses,
        packaging,
        parent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const POM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- generated -->
<project xmlns="http://maven.apache.org/POM/4.0.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
    xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 https://maven.apache.org/xsd/maven-4.0.0.xsd">
  <modelVersion>4.0.0</modelVersion>
  <parent>
    <groupId>org.example</groupId>
    <artifactId>parent</artifactId>
    <version>2.1</version>
  </parent>
  <packaging>aar</packaging>
  <name>Camera &amp; Core</name>
  <url>https://example.org/a?b=1&amp;c=2</url>
  <licenses>
    <license>
      <name>The Apache Software License, Version 2.0</name>
      <url>https://www.apache.org/licenses/LICENSE-2.0.txt</url>
      <distribution>repo</distribution>
    </license>
    <license>
      <name><![CDATA[BSD License]]></name>
      <url>https://chromium.googlesource.com/libyuv/libyuv/</url>
    </license>
  </licenses>
</project>"#;

    #[test]
    fn reads_licences_name_url_packaging_and_parent() {
        let pom = parse_pom(POM).unwrap();
        assert_eq!(pom.name, "Camera & Core");
        assert_eq!(pom.url, "https://example.org/a?b=1&c=2");
        assert_eq!(pom.packaging, "aar");
        assert_eq!(
            pom.parent,
            Some(("org.example".into(), "parent".into(), "2.1".into()))
        );
        assert_eq!(
            pom.licenses,
            [
                PomLicense {
                    name: "The Apache Software License, Version 2.0".into(),
                    url: "https://www.apache.org/licenses/LICENSE-2.0.txt".into()
                },
                PomLicense {
                    name: "BSD License".into(),
                    url: "https://chromium.googlesource.com/libyuv/libyuv/".into()
                },
            ]
        );
    }

    #[test]
    fn missing_fields_default() {
        let pom = parse_pom("<project><artifactId>x</artifactId></project>").unwrap();
        assert_eq!(pom.packaging, "jar");
        assert!(pom.licenses.is_empty());
        assert!(pom.parent.is_none());
        assert_eq!(pom.name, "");
    }

    #[test]
    fn self_closing_elements_and_numeric_entities() {
        let root = parse_xml("<a><b/><c>&#65;&#x42;&bogus;</c></a>").unwrap();
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.child_text("c"), "AB&bogus;");
    }

    #[test]
    fn mismatched_and_unclosed_tags_are_errors() {
        assert!(parse_xml("<a><b></a>").is_err());
        assert!(parse_xml("<a>").is_err());
        assert!(parse_xml("").is_err());
    }
}
