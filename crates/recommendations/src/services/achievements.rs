//! Original NEwBOO reader-achievement thresholds, in ascending order.
//! A chapter is counted only from C's unique, server-validated completion fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AchievementDefinition {
    pub key: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub threshold: i64,
}

pub const THRESHOLDS: [i64; 6] = [1, 10, 50, 100, 250, 500];

pub const READER_ACHIEVEMENTS_V1: [AchievementDefinition; 6] = [
    AchievementDefinition {
        key: "reader_chapters_1",
        title: "Первая глава",
        description: "Завершить первую главу.",
        threshold: 1,
    },
    AchievementDefinition {
        key: "reader_chapters_10",
        title: "Вошёл во вкус",
        description: "Завершить 10 уникальных глав.",
        threshold: 10,
    },
    AchievementDefinition {
        key: "reader_chapters_50",
        title: "Постоянный читатель",
        description: "Завершить 50 уникальных глав.",
        threshold: 50,
    },
    AchievementDefinition {
        key: "reader_chapters_100",
        title: "Сотня глав",
        description: "Завершить 100 уникальных глав.",
        threshold: 100,
    },
    AchievementDefinition {
        key: "reader_chapters_250",
        title: "Книжный марафонец",
        description: "Завершить 250 уникальных глав.",
        threshold: 250,
    },
    AchievementDefinition {
        key: "reader_chapters_500",
        title: "Мастер чтения",
        description: "Завершить 500 уникальных глав.",
        threshold: 500,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_12_ne_wboo_catalog_thresholds_and_keys_are_stable() {
        assert_eq!(
            READER_ACHIEVEMENTS_V1.map(|definition| definition.threshold),
            THRESHOLDS
        );
        for definition in READER_ACHIEVEMENTS_V1 {
            assert_eq!(
                definition.key,
                format!("reader_chapters_{}", definition.threshold)
            );
            assert!(!definition.title.is_empty());
            assert!(!definition.description.is_empty());
        }
        assert_eq!(READER_ACHIEVEMENTS_V1[0].title, "Первая глава");
        assert_eq!(READER_ACHIEVEMENTS_V1[5].title, "Мастер чтения");
    }
}
