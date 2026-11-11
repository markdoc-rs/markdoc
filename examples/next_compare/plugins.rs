//! The tokens stage's way into `markdoc::next::tokenizer`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use markdoc::markdown_it::Token;
use markdoc::next::compare::to_json_string;
use markdoc::next::tokenizer::{Tokenizer, TokenizerOptions};
use serde_json::{Map, Value};

thread_local! {
    /// One Tokenizer per distinct options object, as the oracle keeps one.
    static TOKENIZERS: RefCell<HashMap<String, Rc<Tokenizer>>> = RefCell::new(HashMap::new());
}

/// `new Tokenizer(options).tokenize(src)`. `options` is upstream's Tokenizer config as the
/// request carries it: `allowIndentation`, `allowComments` (compare.py always sets both; the
/// oracle's default for each is true), `allowLinkValidation`, `linkValidationOptions`, and
/// markdown-it's options in its own names (`html`, `typographer`, `breaks`, `quotes`, ...).
/// `Err("Name: message")` where upstream throws.
pub fn tokenize(src: &str, options: &Map<String, Value>) -> Result<Vec<Token>, String> {
    let key = to_json_string(&Value::Object(options.clone()));
    let tokenizer = TOKENIZERS.with(|cache| -> Result<Rc<Tokenizer>, String> {
        if let Some(t) = cache.borrow().get(&key) {
            return Ok(Rc::clone(t));
        }
        let t = Rc::new(Tokenizer::new(TokenizerOptions::from_json(options)?));
        cache.borrow_mut().insert(key.clone(), Rc::clone(&t));
        Ok(t)
    })?;
    tokenizer.tokenize(src).map_err(|e| e.to_string())
}
