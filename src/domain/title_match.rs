use crate::domain::models::EpisodeEntry;

/// An episode and how closely its title appears in some text, from 0.0 (not at all) to 1.0.
#[derive(Debug, Clone)]
pub struct TitleScore {
    pub episode: EpisodeEntry,
    pub score: f64,
}

/// Lowercase letters and digits only. OCR gets spacing, punctuation and quote marks wrong far
/// more often than letters, and a title card's quotes and line breaks aren't part of the title,
/// so none of them are compared.
pub fn normalize(text: &str) -> Vec<char> {
    text.replace('&', " and ")
        .chars()
        .flat_map(char::to_lowercase)
        .map(fold_accent)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// OCR reads "Touché" as "Touche"; the accented letters that turn up in titles are folded to
/// the plain ones.
fn fold_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        c => c,
    }
}

/// The fewest single-character edits that turn `needle` into some substring of `haystack`
/// (Sellers' variant of the edit-distance algorithm: the match may start anywhere for free).
fn substring_distance(needle: &[char], haystack: &[char]) -> usize {
    // column[i] is the distance from the first i characters of `needle` to the best substring
    // ending at the current haystack position
    let mut column: Vec<usize> = (0..=needle.len()).collect();
    let mut best = column[needle.len()];
    for &h in haystack {
        let mut diagonal = column[0];
        column[0] = 0;
        for (i, &n) in needle.iter().enumerate() {
            let above = column[i + 1];
            let substitution = diagonal + usize::from(n != h);
            column[i + 1] = substitution.min(above + 1).min(column[i] + 1);
            diagonal = above;
        }
        best = best.min(column[needle.len()]);
    }
    best
}

/// 1.0 for an exact match of `title` somewhere in `text`, less one for each edit needed as a
/// share of the title's length. The text has other words around the title (credits, studio
/// names), so only the best-matching stretch of it counts.
fn score_normalized(title: &[char], text: &[char]) -> f64 {
    if title.is_empty() {
        return 0.0;
    }
    let distance = substring_distance(title, text);
    1.0 - (distance as f64 / title.len() as f64).min(1.0)
}

#[cfg(test)]
fn title_score(title: &str, text: &str) -> f64 {
    score_normalized(&normalize(title), &normalize(text))
}

/// Scores every episode's title against each of the OCR `texts`, keeping the best reading of
/// it, and returns them best first. Ties keep the order of `episodes`.
pub fn rank_episodes(texts: &[String], episodes: &[EpisodeEntry]) -> Vec<TitleScore> {
    let texts: Vec<Vec<char>> = texts.iter().map(|t| normalize(t)).collect();
    let mut ranked: Vec<TitleScore> = episodes
        .iter()
        .map(|episode| {
            let title = normalize(&episode.name);
            let score = texts
                .iter()
                .map(|text| score_normalized(&title, text))
                .fold(0.0, f64::max);
            TitleScore {
                episode: episode.clone(),
                score,
            }
        })
        .collect();
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score));
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(number: u64, name: &str) -> EpisodeEntry {
        EpisodeEntry {
            production_code: None,
            season_number: 1,
            episode_number: number,
            name: name.to_string(),
            overview: None,
            aired: None,
            runtime: None,
        }
    }

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn normalize_keeps_only_lowercase_letters_and_digits() {
        let normalized: String = normalize("\"Dr. Jekyll & Mr. Mouse\"")
            .into_iter()
            .collect();
        assert_eq!(normalized, "drjekyllandmrmouse");
        let normalized: String = normalize("Touché, Pussy Cat!").into_iter().collect();
        assert_eq!(normalized, "touchepussycat");
    }

    #[test]
    fn substring_distance_finds_the_best_substring() {
        // exact, with other text around it
        assert_eq!(substring_distance(&chars("kitty"), &chars("xxkittyxx")), 0);
        // one substitution, one missing letter
        assert_eq!(substring_distance(&chars("kitty"), &chars("zzkitzy")), 1);
        assert_eq!(substring_distance(&chars("kitty"), &chars("kity")), 1);
        // nothing like it: every character has to change
        assert_eq!(substring_distance(&chars("abc"), &chars("xyz")), 3);
        assert_eq!(substring_distance(&chars("abc"), &[]), 3);
    }

    #[test]
    fn title_score_is_one_for_an_exact_match_and_drops_per_edit() {
        assert_eq!(
            title_score("Kitty Foiled", "KITTY\nFOILED\nDirected by"),
            1.0
        );
        // "kittyfoiled" is 11 letters; "kittyfoled" is one deletion away
        let score = title_score("Kitty Foiled", "Kitty Foled");
        assert!((score - 10.0 / 11.0).abs() < 1e-9);
        assert_eq!(title_score("Kitty Foiled", "zzzzzzzzzzzzzzzz"), 0.0);
        assert_eq!(title_score("!!!", "anything"), 0.0);
    }

    #[test]
    fn rank_episodes_puts_the_closest_title_first() {
        let episodes = vec![
            ep(1, "Trap Happy"),
            ep(2, "Kitty Foiled"),
            ep(3, "Heavenly Puss"),
        ];
        let texts = vec![
            "WILLIAM HANNA JOSEPH BARBERA".to_string(),
            "Kittery fed\nWILLIAM HANNA".to_string(),
        ];
        let ranked = rank_episodes(&texts, &episodes);
        assert_eq!(ranked[0].episode.name, "Kitty Foiled");
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn rank_episodes_keeps_the_best_reading_of_each_title() {
        let episodes = vec![ep(1, "Kitty Foiled")];
        let texts = vec!["garbage".to_string(), "Kitty Foiled".to_string()];
        assert_eq!(rank_episodes(&texts, &episodes)[0].score, 1.0);
    }
}
