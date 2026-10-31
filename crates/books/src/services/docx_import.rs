//! Bounded DOCX text extractor. Never follows external relations or entities.
use crate::services::chapter_file_import::{ImportError, MAX_IMPORT_BYTES};
use roxmltree::{Document, ParsingOptions};
use std::io::{Cursor, Read};

pub const MAX_DOCX_BYTES: usize = 1_250_000;
const MAX_XML_BYTES: usize = 8 * 1024 * 1024;
const MAX_UNCOMPRESSED: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const WORD_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

pub fn valid_docx_filename(name: &str) -> bool {
    name.len() <= 128
        && !name.is_empty()
        && name.to_ascii_lowercase().ends_with(".docx")
        && !name.chars().any(char::is_control)
        && !name.contains('/')
        && !name.contains('\\')
}

fn safe_path(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !(name.len() >= 2 && name.as_bytes()[1] == b':')
        && name
            .trim_end_matches('/')
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

pub fn extract_docx_text(source: &[u8]) -> Result<String, ImportError> {
    if source.is_empty() || source.len() > MAX_DOCX_BYTES {
        return Err(ImportError::TooLarge);
    }
    if !source.starts_with(b"PK\x03\x04") {
        return Err(ImportError::InvalidArchive);
    }
    let mut archive =
        zip::ZipArchive::new(Cursor::new(source)).map_err(|_| ImportError::InvalidArchive)?;
    if archive.is_empty() || archive.len() > MAX_ENTRIES {
        return Err(ImportError::InvalidArchive);
    }
    let (mut total, mut content_types, mut document) = (0_u64, false, None);
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|_| ImportError::InvalidArchive)?;
        let name = file.name();
        if !safe_path(name) || file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(ImportError::InvalidArchive);
        }
        let packed = file.compressed_size();
        if file.size() > MAX_UNCOMPRESSED
            || (packed == 0 && file.size() != 0)
            || (packed != 0 && file.size() / packed > 200)
        {
            return Err(ImportError::TooLarge);
        }
        total = total.saturating_add(file.size());
        if total > MAX_UNCOMPRESSED {
            return Err(ImportError::TooLarge);
        }
        if file.is_dir() {
            continue;
        }
        if name == "[Content_Types].xml" {
            if content_types {
                return Err(ImportError::InvalidArchive);
            }
            content_types = true;
        } else if name == "word/document.xml" {
            if document.is_some() || file.size() > MAX_XML_BYTES as u64 {
                return Err(ImportError::InvalidArchive);
            }
            let mut bytes = Vec::new();
            file.take(MAX_XML_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| ImportError::InvalidArchive)?;
            if bytes.len() > MAX_XML_BYTES {
                return Err(ImportError::TooLarge);
            }
            document = Some(bytes);
        }
    }
    if !content_types {
        return Err(ImportError::InvalidArchive);
    }
    let bytes = document.ok_or(ImportError::InvalidArchive)?;
    let xml = std::str::from_utf8(&bytes).map_err(|_| ImportError::InvalidEncoding)?;
    let doc = Document::parse_with_options(
        xml,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: 200_000,
            ..ParsingOptions::default()
        },
    )
    .map_err(|_| ImportError::InvalidArchive)?;
    let body = doc
        .descendants()
        .find(|node| node.has_tag_name((WORD_NS, "body")))
        .ok_or(ImportError::InvalidArchive)?;
    let mut text = String::new();
    let mut paragraphs = 0;
    for para in body
        .descendants()
        .filter(|n| n.has_tag_name((WORD_NS, "p")))
    {
        paragraphs += 1;
        for node in para.descendants().filter(|n| n.is_element()) {
            if node.has_tag_name((WORD_NS, "t")) {
                let value = node.text().unwrap_or("");
                if text.len().saturating_add(value.len()) > MAX_IMPORT_BYTES {
                    return Err(ImportError::TooLarge);
                }
                text.push_str(value);
            } else if node.has_tag_name((WORD_NS, "tab")) {
                text.push('\t');
            } else if node.has_tag_name((WORD_NS, "br")) {
                text.push('\n');
            }
        }
        text.push('\n');
        if text.len() > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge);
        }
    }
    if paragraphs == 0 || text.trim().is_empty() {
        return Err(ImportError::NoChapters);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::chapter_file_import::parse_text;
    use std::io::Write;

    fn docx(xml: &str, extra: Option<&str>) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("[Content_Types].xml", o).unwrap();
        zip.write_all(b"<Types/>").unwrap();
        if let Some(name) = extra {
            zip.start_file(name, o).unwrap();
            zip.write_all(b"unsafe").unwrap();
        }
        zip.start_file("word/document.xml", o).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    const XML: &str = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
      <w:p><w:r><w:t>Глава 1: Начало</w:t></w:r></w:p>
      <w:p><w:r><w:t>Один &amp; два</w:t></w:r></w:p>
      <w:p><w:r><w:t>Глава 2: Финал</w:t></w:r></w:p>
      <w:p><w:r><w:t>Текст</w:t></w:r></w:p>
    </w:body></w:document>"#;

    #[test]
    fn docx_text_uses_canonical_parser() {
        let chapters = parse_text(
            "extracted.txt",
            &extract_docx_text(&docx(XML, None)).unwrap(),
        )
        .unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].body, "Один & два");
        assert_eq!(chapters[1].title, "Глава 2: Финал");
    }

    #[test]
    fn invalid_archives_and_unsafe_paths_fail_closed() {
        assert_eq!(
            extract_docx_text(b"not zip"),
            Err(ImportError::InvalidArchive)
        );
        assert_eq!(
            extract_docx_text(&docx(XML, Some("../evil.txt"))),
            Err(ImportError::InvalidArchive)
        );
        assert_eq!(
            extract_docx_text(&vec![0; MAX_DOCX_BYTES + 1]),
            Err(ImportError::TooLarge)
        );
    }

    #[test]
    fn dtd_and_entity_references_fail_closed() {
        let xml = r#"<!DOCTYPE w:document [<!ENTITY x "evil">]>
          <w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
          <w:body><w:p><w:r><w:t>&x;</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(
            extract_docx_text(&docx(xml, None)),
            Err(ImportError::InvalidArchive)
        );
    }

    #[test]
    fn expanded_utf8_text_is_bounded() {
        let xml = format!("<w:document xmlns:w=\"{WORD_NS}\"><w:body><w:p><w:t>{}</w:t></w:p></w:body></w:document>", "Ж".repeat(350_001));
        assert_eq!(
            extract_docx_text(&docx(&xml, None)),
            Err(ImportError::TooLarge)
        );
    }
}
