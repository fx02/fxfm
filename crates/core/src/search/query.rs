//! Query parsing, name folding, matching and ranking.

use unicode_normalization::UnicodeNormalization;

/// Folds a name or query word for case- and normalization-insensitive matching: NFC, then lowercase.
pub fn fold(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

/// One glob token of a mask.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Lit(char),
    /// `*`: any run of characters.
    Any,
    /// `?`: exactly one character.
    One,
}

/// A hand-rolled glob over a whole folded name (mirrors `maskRegex` in `ui/lib.mjs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    toks: Vec<Tok>,
    /// For a mask ending in `.*`: the tokens without that suffix, so `name.*` also matches `name`.
    no_ext: Option<Vec<Tok>>,
}

impl Mask {
    fn new(pattern: &str) -> Mask {
        let toks: Vec<Tok> = pattern
            .chars()
            .map(|c| match c {
                '*' => Tok::Any,
                '?' => Tok::One,
                c => Tok::Lit(c),
            })
            .collect();
        let no_ext = if pattern.ends_with(".*") {
            Some(toks[..toks.len() - 2].to_vec())
        } else {
            None
        };
        Mask { toks, no_ext }
    }

    fn is_match(&self, name: &str) -> bool {
        glob(&self.toks, name) || self.no_ext.as_ref().is_some_and(|t| glob(t, name))
    }
}

/// Iterative glob match with single-star backtracking, walking `s` by byte offsets (no allocation).
fn glob(toks: &[Tok], s: &str) -> bool {
    let (mut t, mut i) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while let Some(c) = s[i..].chars().next() {
        match toks.get(t) {
            Some(Tok::Any) => {
                star = Some((t, i));
                t += 1;
            }
            Some(Tok::One) => {
                t += 1;
                i += c.len_utf8();
            }
            Some(Tok::Lit(l)) if *l == c => {
                t += 1;
                i += c.len_utf8();
            }
            _ => match star {
                Some((st, si)) => {
                    let si = si + s[si..].chars().next().map_or(1, char::len_utf8);
                    t = st + 1;
                    i = si;
                    star = Some((st, si));
                }
                None => return false,
            },
        }
    }
    toks[t..].iter().all(|k| *k == Tok::Any)
}

/// One whitespace-separated word of a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// A folded substring.
    Sub(String),
    /// A glob matched against the whole folded name.
    Mask(Mask),
}

/// A parsed search query: every term must match (AND, any order).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    words: Vec<Term>,
}

impl Query {
    /// Splits on whitespace and folds each word; words with `*` or `?` become masks.
    pub fn parse(text: &str) -> Query {
        let words = text
            .split_whitespace()
            .map(|w| {
                let f = fold(w);
                if f.contains(['*', '?']) {
                    Term::Mask(Mask::new(&f))
                } else {
                    Term::Sub(f)
                }
            })
            .collect();
        Query { words }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Minimum class over all terms (2 = starts the name, 1 = starts a word, 0 = inside a word),
    /// or `None` when any term fails. `folded_name` must come from [`fold`].
    pub fn score(&self, folded_name: &str) -> Option<u8> {
        let mut best = 2u8;
        for term in &self.words {
            best = best.min(match term {
                Term::Mask(m) => {
                    if !m.is_match(folded_name) {
                        return None;
                    }
                    2
                }
                Term::Sub(sub) => sub_class(folded_name, sub)?,
            });
        }
        Some(best)
    }
}

/// Best class of any occurrence of `sub` in `name`.
fn sub_class(name: &str, sub: &str) -> Option<u8> {
    let mut best: Option<u8> = None;
    for (pos, _) in name.match_indices(sub) {
        let class = if pos == 0 {
            2
        } else if name[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_alphanumeric())
        {
            1
        } else {
            0
        };
        if class == 2 {
            return Some(2);
        }
        best = Some(best.map_or(class, |b| b.max(class)));
    }
    best
}

/// Keeps the best `limit` hits of (score, mtime, id): score desc, mtime desc, id asc.
/// Returns the total count before truncation.
pub fn rank(hits: &mut Vec<(u8, i64, u32)>, limit: usize) -> usize {
    let total = hits.len();
    let cmp = |a: &(u8, i64, u32), b: &(u8, i64, u32)| {
        b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2))
    };
    if total > limit {
        if limit > 0 {
            hits.select_nth_unstable_by(limit - 1, cmp);
        }
        hits.truncate(limit);
    }
    hits.sort_unstable_by(cmp);
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_normalization::UnicodeNormalization;

    #[test]
    fn and_words_any_order() {
        let q = Query::parse("work scope");
        assert!(q.score(&fold("Scope of Work v2.docx")).is_some());
    }
    #[test]
    fn all_words_required() {
        assert!(Query::parse("scope budget")
            .score(&fold("Scope of Work.docx"))
            .is_none());
    }
    #[test]
    fn nfd_name_matches_nfc_query() {
        let nfd: String = "Pročitaj.txt".nfd().collect();
        assert!(Query::parse("pročitaj").score(&fold(&nfd)).is_some());
    }
    #[test]
    fn mask_word() {
        let q = Query::parse("fxfm *.txt");
        assert!(q.score(&fold("notes-fxfm.txt")).is_some());
        assert!(q.score(&fold("fxfm.md")).is_none());
    }
    #[test]
    fn question_mark_is_one_char() {
        assert!(Query::parse("a?c").score(&fold("abc")).is_some());
        assert!(Query::parse("a?c").score(&fold("abbc")).is_none());
    }
    #[test]
    fn classes() {
        let q = Query::parse("work");
        assert_eq!(q.score("work.txt"), Some(2));
        assert_eq!(q.score("scope of work.txt"), Some(1));
        assert_eq!(q.score("homework.txt"), Some(0));
    }
    #[test]
    fn rank_orders_and_counts() {
        let mut h = vec![(0, 50, 1), (2, 10, 2), (2, 30, 3), (1, 99, 4)];
        assert_eq!(rank(&mut h, 3), 4);
        assert_eq!(h.iter().map(|x| x.2).collect::<Vec<_>>(), vec![3, 2, 4]);
    }
    #[test]
    fn empty_query() {
        assert!(Query::parse("   ").is_empty());
    }
    #[test]
    fn mask_handles_multibyte_chars() {
        assert!(Query::parse("?ič.txt").score("šič.txt").is_some());
        assert!(Query::parse("?ič.txt").score("ššič.txt").is_none());
        assert!(Query::parse("*ž").score("žaž").is_some());
        assert!(Query::parse("*č*j").score("pročitaj").is_some());
        assert!(Query::parse("ž*ž?").score("žaž").is_none());
    }
    #[test]
    fn mask_dot_star_matches_no_extension() {
        assert!(Query::parse("*.*").score("readme").is_some());
        assert!(Query::parse("read.*").score("read").is_some());
        assert!(Query::parse("read.*").score("readme").is_none());
    }
    #[test]
    fn best_class_of_substring_is_used() {
        assert_eq!(Query::parse("work").score("homework work"), Some(1));
    }
}
