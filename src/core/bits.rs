use rust_decimal::Decimal;
use string_cache::DefaultAtom as Atom;

pub type Symbol = Atom; // asset or market name
pub type Amount = Decimal; // price, quantity, value, or rate

// add things like (de)serialization of Amount from string (...when required)