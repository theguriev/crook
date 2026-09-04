//! `when` clauses: the condition a binding is only in force under.
//!
//! VSCode's context expressions, and deliberately the same grammar, because
//! the point of taking a design whole is that what a person already knows
//! transfers. A clause is an expression over *context keys* — names the window
//! answers with a value for at the moment a key is pressed:
//!
//! ```text
//! settingsFocused
//! !searchFocused && paneFocused
//! isMac || platform == 'other'
//! ```
//!
//! # What is here and what is not
//!
//! `&&`, `||`, `!`, parentheses, `==` and `!=`, and a bare key, which is true
//! when its value is truthy. That is the whole of it. VSCode also has `=~`
//! against a regular expression, `in`, and `>=` on numbers; each of those
//! wants a type system this context does not have — every value here is a flag
//! or a short word — and a clause that parses everything and means less than
//! it looks like it means is worse than one that says no.
//!
//! # A clause that does not parse binds nothing
//!
//! The rule the rest of the configuration follows, applied one step further in
//! than the file: a binding whose clause cannot be read is dropped with a line
//! in the log, and the bindings around it are unaffected. The alternative — a
//! clause that fails open — is a chord that fires everywhere on the strength
//! of a typo.

use std::collections::HashMap;

/// What a context key is worth.
///
/// Two kinds, which is what the window actually has to say about itself: a
/// flag, and a short word for the handful of keys that name something rather
/// than answer yes or no.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A yes or a no.
    Flag(bool),
    /// A name, compared against with `==`.
    Word(String),
}

impl Value {
    /// Whether a bare key naming this value reads as true.
    ///
    /// VSCode's rule: everything is true except `false` and the empty string.
    fn is_truthy(&self) -> bool {
        match self {
            Self::Flag(flag) => *flag,
            Self::Word(word) => !word.is_empty() && word != "false",
        }
    }

    /// How `==` sees this value.
    fn as_word(&self) -> &str {
        match self {
            Self::Flag(true) => "true",
            Self::Flag(false) => "false",
            Self::Word(word) => word,
        }
    }
}

/// What the window is doing, as the keys a clause may name.
///
/// Built fresh for each keystroke — see `Workspace::key_context` — because
/// every one of these is a question about the frame the key arrived in, and a
/// cached answer would be the previous frame's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    keys: HashMap<String, Value>,
}

impl Context {
    /// A context that knows nothing, in which every key is false.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a flag.
    #[must_use]
    pub fn with(mut self, key: &str, flag: bool) -> Self {
        self.keys.insert(key.to_owned(), Value::Flag(flag));
        self
    }

    /// Records a key whose value is a word.
    #[must_use]
    pub fn with_word(mut self, key: &str, word: impl Into<String>) -> Self {
        self.keys.insert(key.to_owned(), Value::Word(word.into()));
        self
    }

    /// What this key is worth, or `None` where the window says nothing about
    /// it — which is every key in a clause written for a build that has one
    /// this one does not.
    fn get(&self, key: &str) -> Option<&Value> {
        self.keys.get(key)
    }

    /// Every key the window answers for, sorted, for the settings page to
    /// print — a clause can only be written by somebody who knows the names.
    pub fn keys(&self) -> Vec<(&str, &str)> {
        let mut keys: Vec<(&str, &str)> = self
            .keys
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_word()))
            .collect();
        keys.sort_by_key(|(key, _)| *key);
        keys
    }
}

/// A parsed `when` clause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum When {
    /// A bare key: true while its value is truthy.
    Key(String),
    /// `!clause`.
    Not(Box<When>),
    /// `a && b`, in the order they were written.
    And(Vec<When>),
    /// `a || b`.
    Or(Vec<When>),
    /// `key == word`, and `key != word` with `equal` false.
    Compares {
        /// The context key on the left.
        key: String,
        /// The word on the right, unquoted.
        word: String,
        /// Whether the comparison is `==` rather than `!=`.
        equal: bool,
    },
}

impl When {
    /// Reads a clause, or `None` when it is not one.
    pub fn parse(text: &str) -> Option<Self> {
        let tokens = tokenize(text)?;
        let mut parser = Parser {
            tokens: &tokens,
            at: 0,
        };
        let clause = parser.or()?;
        // A clause with something after it — `a && b)` — is a clause somebody
        // mistyped, and taking the half that parsed would put a binding in
        // force under a condition they did not write.
        parser.finished().then_some(clause)
    }

    /// Whether this clause holds right now.
    pub fn evaluate(&self, context: &Context) -> bool {
        match self {
            Self::Key(key) => context.get(key).is_some_and(Value::is_truthy),
            Self::Not(clause) => !clause.evaluate(context),
            Self::And(clauses) => clauses.iter().all(|clause| clause.evaluate(context)),
            Self::Or(clauses) => clauses.iter().any(|clause| clause.evaluate(context)),
            Self::Compares { key, word, equal } => {
                // A key the window says nothing about compares equal to
                // nothing, which is what makes `!=` true for it: VSCode's
                // `undefined != 'x'`.
                let value = context.get(key).map(Value::as_word).unwrap_or_default();
                (value == word) == *equal
            }
        }
    }

    /// Every context key this clause names, for the settings page.
    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::Key(key) => vec![key.as_str()],
            Self::Compares { key, .. } => vec![key.as_str()],
            Self::Not(clause) => clause.names(),
            Self::And(clauses) | Self::Or(clauses) => {
                clauses.iter().flat_map(When::names).collect()
            }
        }
    }
}

/// One piece of a clause.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    /// A context key, or the word on the right of a comparison.
    Word(String),
    /// `&&`.
    And,
    /// `||`.
    Or,
    /// `!`, as a prefix rather than as the front of `!=`.
    Not,
    /// `==`.
    Equal,
    /// `!=`.
    NotEqual,
    /// `(`.
    Open,
    /// `)`.
    Close,
}

/// Splits a clause into tokens, or `None` at the first character that is not
/// part of one.
fn tokenize(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let characters: Vec<char> = text.chars().collect();
    let mut at = 0;

    while at < characters.len() {
        let character = characters[at];
        match character {
            character if character.is_whitespace() => at += 1,
            '(' => {
                tokens.push(Token::Open);
                at += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                at += 1;
            }
            '&' | '|' => {
                // Both are two characters, and a single one is not an
                // operator: `a & b` is a clause somebody meant to write with
                // two, and reading it as one would be guessing.
                if characters.get(at + 1) != Some(&character) {
                    return None;
                }
                tokens.push(if character == '&' {
                    Token::And
                } else {
                    Token::Or
                });
                at += 2;
            }
            '!' => {
                if characters.get(at + 1) == Some(&'=') {
                    tokens.push(Token::NotEqual);
                    at += 2;
                } else {
                    tokens.push(Token::Not);
                    at += 1;
                }
            }
            '=' => {
                // `=~` is VSCode's regular-expression match, which this does
                // not have; a single `=` is nothing at all.
                if characters.get(at + 1) != Some(&'=') {
                    return None;
                }
                tokens.push(Token::Equal);
                at += 2;
            }
            '\'' | '"' => {
                let quote = character;
                at += 1;
                let start = at;
                while at < characters.len() && characters[at] != quote {
                    at += 1;
                }
                if at == characters.len() {
                    return None;
                }
                tokens.push(Token::Word(characters[start..at].iter().collect()));
                at += 1;
            }
            character if is_word(character) => {
                let start = at;
                while at < characters.len() && is_word(characters[at]) {
                    at += 1;
                }
                tokens.push(Token::Word(characters[start..at].iter().collect()));
            }
            _ => return None,
        }
    }

    Some(tokens)
}

/// Whether a character can be part of a context key or an unquoted word.
///
/// The dots and colons are there because VSCode's own keys have them —
/// `editorLangId`, `terminal.active` — and a person moving a clause across
/// should not have to find out which punctuation this one refuses.
fn is_word(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | ':')
}

/// A hand-written recursive descent over the tokens, lowest precedence first:
/// `||`, then `&&`, then `!`, then a key or a comparison.
struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
}

impl Parser<'_> {
    fn finished(&self) -> bool {
        self.at == self.tokens.len()
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn take(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            return true;
        }
        false
    }

    fn or(&mut self) -> Option<When> {
        let mut clauses = vec![self.and()?];
        while self.take(&Token::Or) {
            clauses.push(self.and()?);
        }
        Some(if clauses.len() == 1 {
            clauses.remove(0)
        } else {
            When::Or(clauses)
        })
    }

    fn and(&mut self) -> Option<When> {
        let mut clauses = vec![self.unary()?];
        while self.take(&Token::And) {
            clauses.push(self.unary()?);
        }
        Some(if clauses.len() == 1 {
            clauses.remove(0)
        } else {
            When::And(clauses)
        })
    }

    fn unary(&mut self) -> Option<When> {
        if self.take(&Token::Not) {
            return Some(When::Not(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Option<When> {
        if self.take(&Token::Open) {
            let clause = self.or()?;
            return self.take(&Token::Close).then_some(clause);
        }

        let Token::Word(key) = self.peek()?.clone() else {
            return None;
        };
        self.at += 1;

        let equal = match self.peek() {
            Some(Token::Equal) => true,
            Some(Token::NotEqual) => false,
            // A bare key, which is the ordinary shape of a clause.
            _ => return Some(When::Key(key)),
        };
        self.at += 1;

        let Token::Word(word) = self.peek()?.clone() else {
            return None;
        };
        self.at += 1;

        Some(When::Compares { key, word, equal })
    }
}

#[cfg(test)]
#[path = "when_tests.rs"]
mod tests;
