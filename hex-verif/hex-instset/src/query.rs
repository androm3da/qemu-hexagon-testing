//! English predicate query expression parser for instruction filtering.
//!
//! Parses expressions like `"is solo or is call"` into [`Filter`] AST
//! nodes that can be evaluated against any
//! [`InstructionDef`](hex_dump::types::InstructionDef).
//!
//! # Grammar
//!
//! ```text
//! expr     ::= or_expr
//! or_expr  ::= and_expr ("or" and_expr)*
//! and_expr ::= atom ("and" atom)*
//! atom     ::= "not" atom
//!            | "(" expr ")"
//!            | predicate
//!
//! predicate ::= "is" ATTR
//!             | "may" VERB
//!             | "has" PROPERTY
//!             | "type" "is" TYPE_NAME
//!             | "requires" FEATURE
//!             | "has" REG_CLASS "operand"
//!             | "has" REG_CLASS "input"
//!             | "has" REG_CLASS "output"
//!             | "has" "immediate" "operand"
//!             | "name" "contains" WORD
//!             | "syntax" "contains" WORD
//! ```
//!
//! # Predicate vocabulary
//!
//! ## `is` -- boolean attribute predicates
//!
//! | Expression | `InstructionDef` field |
//! |---|---|
//! | `is solo` | `is_solo` |
//! | `is soloax` | `is_solo_ax` |
//! | `is predicated` | `is_predicated` |
//! | `is predicated_new` | `is_predicated_new` |
//! | `is nv_store` | `is_nv_store` |
//! | `is cvi` | `is_cvi` |
//! | `is hvx_alu` | `is_hvx_alu` |
//! | `is commutable` | `is_commutable` |
//! | `is predicable` | `is_predicable` |
//! | `is extendable` | `is_extendable` |
//! | `is fp` | `is_fp` |
//! | `is call` | `is_call` |
//! | `is return` | `is_return` |
//!
//! ## `may` -- capability predicates
//!
//! | Expression | `InstructionDef` field |
//! |---|---|
//! | `may load` | `may_load` |
//! | `may store` | `may_store` |
//!
//! ## `has` -- property and operand predicates
//!
//! | Expression | Meaning |
//! |---|---|
//! | `has new_value` | `has_new_value` is true |
//! | `has side_effects` | `has_side_effects` is true |
//! | `has immediate operand` | Any operand is an immediate |
//! | `has <RC> operand` | Any operand uses register class `<RC>` |
//! | `has <RC> input` | An input operand uses register class `<RC>` |
//! | `has <RC> output` | An output operand uses register class `<RC>` |
//!
//! Valid register class names for `<RC>`: `IntRegs`, `DoubleRegs`,
//! `PredRegs`, `HvxVR`, `HvxWR`, `HvxQR`, `ModRegs`, `CtrRegs`,
//! `CtrRegs64`, `GuestRegs`, `GuestRegs64`.
//!
//! ## Other predicates
//!
//! | Expression | Meaning |
//! |---|---|
//! | `type is <TYPE>` | Instruction type equals `<TYPE>` (e.g., `TypeALU32_3op`) |
//! | `requires <FEATURE>` | `requires` list contains `<FEATURE>` (substring) |
//! | `name contains <WORD>` | Instruction name contains `<WORD>` (case-insensitive) |
//! | `syntax contains <WORD>` | Assembly syntax contains `<WORD>` (case-insensitive) |
//!
//! Multi-word arguments can be quoted: `syntax contains ":sat"`.
//!
//! # Examples
//!
//! ```
//! use hex_instset::query::parse_query;
//!
//! // Simple predicate
//! let f = parse_query("is solo").unwrap();
//!
//! // Boolean combinators
//! let f = parse_query("is solo or is call or is return").unwrap();
//!
//! // Negation
//! let f = parse_query("not is cvi").unwrap();
//!
//! // Parenthesized grouping
//! let f = parse_query("(may load or may store) and not is hvx_alu").unwrap();
//!
//! // Register class queries
//! let f = parse_query("has HvxVR operand or has HvxWR operand").unwrap();
//!
//! // Syntax substring match
//! let f = parse_query("syntax contains :sat").unwrap();
//!
//! // Parse errors are reported with position information.
//! let err = parse_query("is bogus").unwrap_err();
//! assert!(err.message.contains("unknown attribute"));
//! ```

use std::fmt;

use crate::filter::{AttributeFilter, Filter, OperandDir};

/// Error type for query expression parsing.
#[derive(Debug, Clone)]
pub struct QueryParseError {
    pub message: String,
    pub position: usize,
}

impl fmt::Display for QueryParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "parse error at position {}: {}",
            self.position, self.message
        )
    }
}

impl std::error::Error for QueryParseError {}

/// Parse an English predicate query expression into a Filter.
pub fn parse_query(input: &str) -> Result<Filter, QueryParseError> {
    let tokens = tokenize(input)?;
    let mut parser = Parser::new(&tokens);
    let filter = parser.parse_or()?;
    if parser.pos < parser.tokens.len() {
        return Err(QueryParseError {
            message: format!("unexpected token: '{}'", parser.tokens[parser.pos].text),
            position: parser.tokens[parser.pos].offset,
        });
    }
    Ok(filter)
}

#[derive(Debug, Clone)]
struct Token {
    text: String,
    offset: usize,
}

/// Tokenize input, splitting on whitespace but keeping `(` and `)` as separate tokens.
/// Quoted strings (single or double quotes) are kept as a single token (without quotes).
fn tokenize(input: &str) -> Result<Vec<Token>, QueryParseError> {
    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some(&(i, ch)) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch == '(' || ch == ')' {
            tokens.push(Token {
                text: ch.to_string(),
                offset: i,
            });
            chars.next();
            continue;
        }
        if ch == '"' || ch == '\'' {
            let quote = ch;
            chars.next(); // consume opening quote
            let start = i + 1;
            let mut end = start;
            let mut found_close = false;
            while let Some(&(j, c)) = chars.peek() {
                if c == quote {
                    end = j;
                    found_close = true;
                    chars.next(); // consume closing quote
                    break;
                }
                end = j + c.len_utf8();
                chars.next();
            }
            if !found_close {
                return Err(QueryParseError {
                    message: "unterminated quoted string".to_string(),
                    position: i,
                });
            }
            tokens.push(Token {
                text: input[start..end].to_string(),
                offset: start,
            });
            continue;
        }
        // Regular word
        let start = i;
        let mut end = start;
        while let Some(&(j, c)) = chars.peek() {
            if c.is_whitespace() || c == '(' || c == ')' {
                break;
            }
            end = j + c.len_utf8();
            chars.next();
        }
        tokens.push(Token {
            text: input[start..end].to_string(),
            offset: start,
        });
    }

    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(|t| t.text.as_str())
    }

    fn advance(&mut self) -> Result<&Token, QueryParseError> {
        if self.pos >= self.tokens.len() {
            return Err(QueryParseError {
                message: "unexpected end of expression".to_string(),
                position: self
                    .tokens
                    .last()
                    .map(|t| t.offset + t.text.len())
                    .unwrap_or(0),
            });
        }
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        Ok(tok)
    }

    fn expect(&mut self, expected: &str) -> Result<(), QueryParseError> {
        let tok = self.advance()?;
        if tok.text != expected {
            return Err(QueryParseError {
                message: format!("expected '{}', got '{}'", expected, tok.text),
                position: tok.offset,
            });
        }
        Ok(())
    }

    fn current_offset(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map(|t| t.offset)
            .unwrap_or_else(|| {
                self.tokens
                    .last()
                    .map(|t| t.offset + t.text.len())
                    .unwrap_or(0)
            })
    }

    /// Parse: or_expr ::= and_expr ("or" and_expr)*
    fn parse_or(&mut self) -> Result<Filter, QueryParseError> {
        let mut left = self.parse_and()?;
        while self.peek() == Some("or") {
            self.advance()?; // consume "or"
            let right = self.parse_and()?;
            left = match left {
                Filter::Or(mut v) => {
                    v.push(right);
                    Filter::Or(v)
                }
                _ => Filter::Or(vec![left, right]),
            };
        }
        Ok(left)
    }

    /// Parse: and_expr ::= atom ("and" atom)*
    fn parse_and(&mut self) -> Result<Filter, QueryParseError> {
        let mut left = self.parse_atom()?;
        while self.peek() == Some("and") {
            self.advance()?; // consume "and"
            let right = self.parse_atom()?;
            left = match left {
                Filter::And(mut v) => {
                    v.push(right);
                    Filter::And(v)
                }
                _ => Filter::And(vec![left, right]),
            };
        }
        Ok(left)
    }

    /// Parse: atom ::= "not" atom | "(" expr ")" | predicate
    fn parse_atom(&mut self) -> Result<Filter, QueryParseError> {
        match self.peek() {
            Some("not") => {
                self.advance()?;
                let inner = self.parse_atom()?;
                Ok(Filter::Not(Box::new(inner)))
            }
            Some("(") => {
                self.advance()?;
                let inner = self.parse_or()?;
                self.expect(")")?;
                Ok(inner)
            }
            Some(_) => self.parse_predicate(),
            None => Err(QueryParseError {
                message: "unexpected end of expression".to_string(),
                position: self.current_offset(),
            }),
        }
    }

    /// Parse a predicate expression.
    fn parse_predicate(&mut self) -> Result<Filter, QueryParseError> {
        let tok = self.advance()?;
        let keyword = tok.text.clone();
        let offset = tok.offset;

        match keyword.as_str() {
            "is" => self.parse_is_predicate(),
            "may" => self.parse_may_predicate(),
            "has" => self.parse_has_predicate(),
            "type" => {
                self.expect("is")?;
                let type_tok = self.advance()?;
                Ok(Filter::ByType(type_tok.text.clone()))
            }
            "requires" => {
                let feature_tok = self.advance()?;
                Ok(Filter::ByRequires(feature_tok.text.clone()))
            }
            "name" => {
                self.expect("contains")?;
                let word_tok = self.advance()?;
                Ok(Filter::ByNameContains(word_tok.text.clone()))
            }
            "syntax" => {
                self.expect("contains")?;
                let word_tok = self.advance()?;
                Ok(Filter::BySyntaxContains(word_tok.text.clone()))
            }
            _ => Err(QueryParseError {
                message: format!("unknown predicate keyword: '{}'", keyword),
                position: offset,
            }),
        }
    }

    /// Parse: "is" ATTR -> Filter::ByAttribute
    fn parse_is_predicate(&mut self) -> Result<Filter, QueryParseError> {
        let tok = self.advance()?;
        let attr = tok.text.clone();
        let offset = tok.offset;

        let filter = match attr.as_str() {
            "solo" => AttributeFilter::IsSolo(true),
            "soloax" => AttributeFilter::IsSoloAX(true),
            "predicated" => AttributeFilter::IsPredicated(true),
            "predicated_new" => AttributeFilter::IsPredicatedNew(true),
            "nv_store" => AttributeFilter::IsNvStore(true),
            "cvi" => AttributeFilter::IsCvi(true),
            "hvx_alu" => AttributeFilter::IsHvxAlu(true),
            "commutable" => AttributeFilter::IsCommutable(true),
            "predicable" => AttributeFilter::IsPredicable(true),
            "extendable" => AttributeFilter::IsExtendable(true),
            "fp" => AttributeFilter::IsFp(true),
            "call" => AttributeFilter::IsCall(true),
            "return" => AttributeFilter::IsReturn(true),
            _ => {
                return Err(QueryParseError {
                    message: format!("unknown attribute for 'is': '{}'", attr),
                    position: offset,
                });
            }
        };
        Ok(Filter::ByAttribute(filter))
    }

    /// Parse: "may" VERB -> Filter::ByAttribute
    fn parse_may_predicate(&mut self) -> Result<Filter, QueryParseError> {
        let tok = self.advance()?;
        let verb = tok.text.clone();
        let offset = tok.offset;

        let filter = match verb.as_str() {
            "load" => AttributeFilter::MayLoad(true),
            "store" => AttributeFilter::MayStore(true),
            _ => {
                return Err(QueryParseError {
                    message: format!("unknown verb for 'may': '{}'", verb),
                    position: offset,
                });
            }
        };
        Ok(Filter::ByAttribute(filter))
    }

    /// Parse: "has" PROPERTY | "has" REG_CLASS "operand"/"input"/"output" | "has" "immediate" "operand"
    fn parse_has_predicate(&mut self) -> Result<Filter, QueryParseError> {
        let tok = self.advance()?;
        let word = tok.text.clone();
        let offset = tok.offset;

        // Check for known boolean properties first
        match word.as_str() {
            "new_value" => return Ok(Filter::ByAttribute(AttributeFilter::HasNewValue(true))),
            "side_effects" => {
                return Ok(Filter::ByAttribute(AttributeFilter::HasSideEffects(true)));
            }
            "immediate" => {
                self.expect("operand")?;
                return Ok(Filter::HasImmediateOperand);
            }
            _ => {}
        }

        // Check if it's a register class name followed by operand/input/output
        let known_reg_classes = [
            "IntRegs",
            "DoubleRegs",
            "PredRegs",
            "HvxVR",
            "HvxWR",
            "HvxQR",
            "ModRegs",
            "CtrRegs",
            "CtrRegs64",
            "GuestRegs",
            "GuestRegs64",
        ];

        if known_reg_classes.contains(&word.as_str()) {
            let dir_tok = self.advance()?;
            let direction = match dir_tok.text.as_str() {
                "operand" => OperandDir::Any,
                "input" => OperandDir::Input,
                "output" => OperandDir::Output,
                _ => {
                    return Err(QueryParseError {
                        message: format!(
                            "expected 'operand', 'input', or 'output' after register class, got '{}'",
                            dir_tok.text
                        ),
                        position: dir_tok.offset,
                    });
                }
            };
            return Ok(Filter::HasRegClassOperand {
                class: word,
                direction,
            });
        }

        Err(QueryParseError {
            message: format!(
                "unknown property for 'has': '{}' (expected new_value, side_effects, immediate, or a register class name)",
                word
            ),
            position: offset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_dump::types::{InstructionDef, Operand};

    #[test]
    fn test_parse_is_predicate() {
        let f = parse_query("is solo").unwrap();
        assert!(matches!(
            f,
            Filter::ByAttribute(AttributeFilter::IsSolo(true))
        ));
    }

    #[test]
    fn test_parse_may_predicate() {
        let f = parse_query("may load").unwrap();
        assert!(matches!(
            f,
            Filter::ByAttribute(AttributeFilter::MayLoad(true))
        ));
    }

    #[test]
    fn test_parse_has_property() {
        let f = parse_query("has side_effects").unwrap();
        assert!(matches!(
            f,
            Filter::ByAttribute(AttributeFilter::HasSideEffects(true))
        ));
    }

    #[test]
    fn test_parse_type_is() {
        let f = parse_query("type is TypeALU32_3op").unwrap();
        match f {
            Filter::ByType(t) => assert_eq!(t, "TypeALU32_3op"),
            _ => panic!("expected ByType"),
        }
    }

    #[test]
    fn test_parse_and_or() {
        let f = parse_query("is solo or is call").unwrap();
        match f {
            Filter::Or(v) => {
                assert_eq!(v.len(), 2);
                assert!(matches!(
                    v[0],
                    Filter::ByAttribute(AttributeFilter::IsSolo(true))
                ));
                assert!(matches!(
                    v[1],
                    Filter::ByAttribute(AttributeFilter::IsCall(true))
                ));
            }
            _ => panic!("expected Or"),
        }
    }

    #[test]
    fn test_parse_not() {
        let f = parse_query("not is solo").unwrap();
        match f {
            Filter::Not(inner) => {
                assert!(matches!(
                    *inner,
                    Filter::ByAttribute(AttributeFilter::IsSolo(true))
                ));
            }
            _ => panic!("expected Not"),
        }
    }

    #[test]
    fn test_parse_parens() {
        let f = parse_query("(is solo or is call) and not has side_effects").unwrap();
        match f {
            Filter::And(v) => {
                assert_eq!(v.len(), 2);
                assert!(matches!(&v[0], Filter::Or(_)));
                assert!(matches!(&v[1], Filter::Not(_)));
            }
            _ => panic!("expected And"),
        }
    }

    #[test]
    fn test_parse_has_regclass() {
        let f = parse_query("has HvxVR operand").unwrap();
        match f {
            Filter::HasRegClassOperand { class, direction } => {
                assert_eq!(class, "HvxVR");
                assert_eq!(direction, OperandDir::Any);
            }
            _ => panic!("expected HasRegClassOperand"),
        }
    }

    #[test]
    fn test_parse_has_regclass_input() {
        let f = parse_query("has IntRegs input").unwrap();
        match f {
            Filter::HasRegClassOperand { class, direction } => {
                assert_eq!(class, "IntRegs");
                assert_eq!(direction, OperandDir::Input);
            }
            _ => panic!("expected HasRegClassOperand"),
        }
    }

    #[test]
    fn test_parse_has_regclass_output() {
        let f = parse_query("has DoubleRegs output").unwrap();
        match f {
            Filter::HasRegClassOperand { class, direction } => {
                assert_eq!(class, "DoubleRegs");
                assert_eq!(direction, OperandDir::Output);
            }
            _ => panic!("expected HasRegClassOperand"),
        }
    }

    #[test]
    fn test_parse_requires() {
        let f = parse_query("requires UseAudio").unwrap();
        match f {
            Filter::ByRequires(s) => assert_eq!(s, "UseAudio"),
            _ => panic!("expected ByRequires"),
        }
    }

    #[test]
    fn test_parse_name_contains() {
        let f = parse_query("name contains A2_add").unwrap();
        match f {
            Filter::ByNameContains(s) => assert_eq!(s, "A2_add"),
            _ => panic!("expected ByNameContains"),
        }
    }

    #[test]
    fn test_parse_syntax_contains() {
        let f = parse_query("syntax contains :sat").unwrap();
        match f {
            Filter::BySyntaxContains(s) => assert_eq!(s, ":sat"),
            _ => panic!("expected BySyntaxContains"),
        }
    }

    #[test]
    fn test_parse_has_immediate_operand() {
        let f = parse_query("has immediate operand").unwrap();
        assert!(matches!(f, Filter::HasImmediateOperand));
    }

    #[test]
    fn test_parse_complex_expression() {
        let f = parse_query("(may load or may store) and not is hvx_alu").unwrap();
        match f {
            Filter::And(v) => {
                assert_eq!(v.len(), 2);
                match &v[0] {
                    Filter::Or(inner) => {
                        assert_eq!(inner.len(), 2);
                    }
                    _ => panic!("expected Or"),
                }
            }
            _ => panic!("expected And"),
        }
    }

    #[test]
    fn test_parse_triple_or() {
        let f = parse_query("is solo or is call or is return").unwrap();
        match f {
            Filter::Or(v) => assert_eq!(v.len(), 3),
            _ => panic!("expected Or with 3 elements"),
        }
    }

    #[test]
    fn test_parse_quoted_string() {
        let f = parse_query("syntax contains \":sat\"").unwrap();
        match f {
            Filter::BySyntaxContains(s) => assert_eq!(s, ":sat"),
            _ => panic!("expected BySyntaxContains"),
        }
    }

    #[test]
    fn test_parse_error_unknown_keyword() {
        let result = parse_query("foo bar");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_empty() {
        let result = parse_query("");
        assert!(result.is_err());
    }

    #[test]
    fn test_eval_on_instruction() {
        let mut insn = InstructionDef::new("A2_add".to_string());
        insn.itype = "TypeALU32_3op".to_string();
        insn.asm_syntax = "$Rd32 = add($Rs32,$Rt32)".to_string();
        insn.has_new_value = true;
        insn.is_solo = false;
        insn.is_call = false;
        insn.outs = vec![Operand {
            name: "Rd32".to_string(),
            reg_class: Some("IntRegs".to_string()),
            is_immediate: false,
            imm_type: None,
        }];
        insn.ins = vec![
            Operand {
                name: "Rs32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
            Operand {
                name: "Rt32".to_string(),
                reg_class: Some("IntRegs".to_string()),
                is_immediate: false,
                imm_type: None,
            },
        ];

        // Should match
        let f = parse_query("type is TypeALU32_3op").unwrap();
        assert!(f.matches(&insn));

        // Should not match
        let f = parse_query("is solo").unwrap();
        assert!(!f.matches(&insn));

        // Compound expression
        let f = parse_query("has new_value and not is call").unwrap();
        assert!(f.matches(&insn));

        // Name contains
        let f = parse_query("name contains A2_add").unwrap();
        assert!(f.matches(&insn));

        // Reg class operand
        let f = parse_query("has IntRegs operand").unwrap();
        assert!(f.matches(&insn));

        let f = parse_query("has HvxVR operand").unwrap();
        assert!(!f.matches(&insn));

        let f = parse_query("has IntRegs output").unwrap();
        assert!(f.matches(&insn));

        let f = parse_query("has IntRegs input").unwrap();
        assert!(f.matches(&insn));
    }
}
