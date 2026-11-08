//! HTML-facing public catalogue taxonomy; never carries PostgreSQL adapter types.
#![forbid(unsafe_code)]

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogTaxonomyView {
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub fandoms: Vec<String>,
}

impl CatalogTaxonomyView {
    pub fn new(genres: Vec<String>, tags: Vec<String>, fandoms: Vec<String>) -> Self {
        Self {
            genres,
            tags,
            fandoms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a01_taxonomy_view_preserves_fields_and_labels() {
        let view = CatalogTaxonomyView::new(
            vec!["Фэнтези".to_owned()],
            vec!["Перевод".to_owned()],
            vec!["Аниме".to_owned()],
        );
        assert_eq!(view.genres, vec!["Фэнтези"]);
        assert_eq!(view.tags, vec!["Перевод"]);
        assert_eq!(view.fandoms, vec!["Аниме"]);
    }
}
