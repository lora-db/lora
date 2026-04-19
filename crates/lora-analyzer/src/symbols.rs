use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VarId(pub u32);

impl fmt::Display for VarId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Default)]
pub struct SymbolTable {
    next_var: u32,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self { next_var: 0 }
    }

    pub fn new_var(&mut self) -> VarId {
        let id = VarId(self.next_var);
        self.next_var += 1;
        id
    }
}