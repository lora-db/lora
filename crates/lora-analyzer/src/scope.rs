use crate::symbols::VarId;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Default)]
pub struct Scope {
    vars: HashMap<String, VarId>,
}

impl Scope {
    pub fn new() -> Self {
        Self {
            vars: HashMap::new(),
        }
    }

    pub fn declare(&mut self, name: String, id: VarId) {
        self.vars.insert(name, id);
    }

    pub fn resolve(&self, name: &str) -> Option<VarId> {
        self.vars.get(name).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &VarId)> {
        self.vars.iter()
    }

    pub fn clear(&mut self) {
        self.vars.clear();
    }

    pub fn contains(&self, name: &str) -> bool {
        self.vars.contains_key(name)
    }
}

#[derive(Debug, Clone)]
pub struct ScopeStack {
    scopes: Vec<Scope>,
}

impl Default for ScopeStack {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeStack {
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope::new()],
        }
    }

    pub fn push(&mut self) {
        self.scopes.push(Scope::new());
    }

    pub fn pop(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        } else if let Some(scope) = self.scopes.last_mut() {
            scope.clear();
        }
    }

    pub fn clear(&mut self) {
        self.scopes.clear();
        self.scopes.push(Scope::new());
    }

    pub fn declare(&mut self, name: String, id: VarId) {
        if self.scopes.is_empty() {
            self.scopes.push(Scope::new());
        }

        if let Some(scope) = self.scopes.last_mut() {
            scope.declare(name, id);
        }
    }

    pub fn resolve(&self, name: &str) -> Option<VarId> {
        for scope in self.scopes.iter().rev() {
            if let Some(id) = scope.resolve(name) {
                return Some(id);
            }
        }
        None
    }

    pub fn contains_in_current_scope(&self, name: &str) -> bool {
        self.scopes
            .last()
            .map(|scope| scope.contains(name))
            .unwrap_or(false)
    }

    pub fn visible_bindings(&self) -> BTreeMap<String, VarId> {
        let mut out = BTreeMap::new();

        for scope in &self.scopes {
            for (name, id) in scope.iter() {
                out.insert(name.clone(), *id);
            }
        }

        out
    }
}
