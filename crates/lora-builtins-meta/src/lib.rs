//! Metadata table for lora's namespaced builtin functions.
//!
//! Owned by this leaf crate so the analyzer (which validates function
//! references at compile time), the executor (which dispatches on the
//! `op` strings), and the editor's WASM bridge (which feeds the
//! autocomplete / signature-hint surface) can all share a single
//! declaration. Drift-safety tests in lora-executor assert that every
//! entry here has a dispatch arm.
//!
//! This is intentionally more than an arity table: enum-like argument
//! slots live here too, so analyzer rewrites and executor dispatch
//! share one declaration for each builtin.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinSpec {
    pub name: &'static str,
    pub arity: Arity,
    pub enum_arg_slots: &'static [usize],
    pub type_arg_slots: &'static [usize],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinAlias {
    pub alias: &'static str,
    pub canonical: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arity {
    pub min: usize,
    pub max: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionId {
    Builtin(&'static BuiltinSpec),
    Aggregate(AggregateFunction),
}

impl FunctionId {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            FunctionId::Builtin(spec) => spec.name,
            FunctionId::Aggregate(function) => function.name(),
        }
    }

    #[must_use]
    pub const fn arity(self) -> Arity {
        match self {
            FunctionId::Builtin(spec) => spec.arity,
            FunctionId::Aggregate(function) => function.arity(),
        }
    }

    #[must_use]
    pub const fn is_aggregate(self) -> bool {
        matches!(self, FunctionId::Aggregate(_))
    }

    #[must_use]
    pub fn eq_ignore_ascii_case(self, other: &str) -> bool {
        self.name().eq_ignore_ascii_case(other)
    }

    #[must_use]
    pub fn to_ascii_lowercase(self) -> String {
        self.name().to_ascii_lowercase()
    }

    #[must_use]
    pub const fn as_aggregate(self) -> Option<AggregateFunction> {
        match self {
            FunctionId::Aggregate(function) => Some(function),
            FunctionId::Builtin(_) => None,
        }
    }

    #[must_use]
    pub fn builtin(name: &str) -> Option<Self> {
        builtin_spec(name).map(FunctionId::Builtin)
    }
}

impl std::fmt::Display for FunctionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateFunction {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
    Stdev,
    Stdevp,
    PercentileCont,
    PercentileDisc,
}

impl AggregateFunction {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            AggregateFunction::Count => "count",
            AggregateFunction::Sum => "sum",
            AggregateFunction::Avg => "avg",
            AggregateFunction::Min => "min",
            AggregateFunction::Max => "max",
            AggregateFunction::Collect => "collect",
            AggregateFunction::Stdev => "stdev",
            AggregateFunction::Stdevp => "stdevp",
            AggregateFunction::PercentileCont => "percentilecont",
            AggregateFunction::PercentileDisc => "percentiledisc",
        }
    }

    #[must_use]
    pub const fn arity(self) -> Arity {
        match self {
            AggregateFunction::Count => Arity {
                min: 0,
                max: Some(1),
            },
            AggregateFunction::Sum
            | AggregateFunction::Avg
            | AggregateFunction::Min
            | AggregateFunction::Max
            | AggregateFunction::Collect
            | AggregateFunction::Stdev
            | AggregateFunction::Stdevp => Arity {
                min: 1,
                max: Some(1),
            },
            AggregateFunction::PercentileCont | AggregateFunction::PercentileDisc => Arity {
                min: 2,
                max: Some(2),
            },
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "count" => Self::Count,
            "sum" => Self::Sum,
            "avg" => Self::Avg,
            "min" => Self::Min,
            "max" => Self::Max,
            "collect" => Self::Collect,
            "stdev" => Self::Stdev,
            "stdevp" => Self::Stdevp,
            "percentilecont" => Self::PercentileCont,
            "percentiledisc" => Self::PercentileDisc,
            _ => return None,
        })
    }
}

const fn spec(name: &'static str, min: usize, max: Option<usize>) -> BuiltinSpec {
    BuiltinSpec {
        name,
        arity: Arity { min, max },
        enum_arg_slots: &[],
        type_arg_slots: &[],
    }
}

const fn spec_enum(
    name: &'static str,
    min: usize,
    max: Option<usize>,
    enum_arg_slots: &'static [usize],
) -> BuiltinSpec {
    BuiltinSpec {
        name,
        arity: Arity { min, max },
        enum_arg_slots,
        type_arg_slots: &[],
    }
}

const fn spec_type(
    name: &'static str,
    min: usize,
    max: Option<usize>,
    type_arg_slots: &'static [usize],
) -> BuiltinSpec {
    BuiltinSpec {
        name,
        arity: Arity { min, max },
        enum_arg_slots: &[],
        type_arg_slots,
    }
}

const fn alias(alias: &'static str, canonical: &'static str) -> BuiltinAlias {
    BuiltinAlias { alias, canonical }
}

pub const BUILTIN_SPECS: &[BuiltinSpec] = &[
    // -- list.* -------------------------------------------------------------
    spec("list.sum", 1, Some(1)),
    spec("list.avg", 1, Some(1)),
    spec("list.min", 1, Some(1)),
    spec("list.max", 1, Some(1)),
    spec("list.product", 1, Some(1)),
    spec("list.stdev", 1, Some(1)),
    spec("list.median", 1, Some(1)),
    spec("list.sort", 1, Some(2)),
    spec("list.reverse", 1, Some(1)),
    spec("list.unique", 1, Some(1)),
    spec("list.first", 1, Some(1)),
    spec("list.rest", 1, Some(1)),
    spec("list.init", 1, Some(1)),
    spec("list.last", 1, Some(1)),
    spec("list.at", 2, Some(2)),
    spec("list.slice", 2, Some(3)),
    spec("list.size", 1, Some(1)),
    spec("list.range", 2, Some(3)),
    spec("list.contains", 2, Some(2)),
    spec("list.contains_all", 2, Some(2)),
    spec("list.has_duplicates", 1, Some(1)),
    spec("list.all_distinct", 1, Some(1)),
    spec("list.equal_unordered", 2, Some(2)),
    spec("list.is_empty", 1, Some(1)),
    spec("list.index_of", 2, Some(2)),
    spec("list.indexes_of", 2, Some(2)),
    spec("list.find_duplicates", 1, Some(1)),
    spec("list.count_by", 1, Some(1)),
    spec("list.union", 2, Some(2)),
    spec("list.intersect", 2, Some(2)),
    spec("list.diff", 2, Some(2)),
    spec("list.symmetric_diff", 2, Some(2)),
    spec("list.zip", 2, Some(2)),
    spec("list.chunks", 2, Some(2)),
    spec("list.split_by", 2, Some(2)),
    spec("list.windows", 2, Some(3)),
    spec("list.scan", 2, Some(2)),
    spec("list.repeat", 2, Some(2)),
    spec("list.flatten", 1, Some(2)),
    spec("list.sample", 1, Some(2)),
    spec("list.shuffle", 1, Some(1)),
    spec("list.combinations", 2, Some(2)),
    spec("list.concat", 2, None),
    spec("list.append", 2, Some(2)),
    spec("list.prepend", 2, Some(2)),
    spec("list.take", 2, Some(2)),
    spec("list.drop", 2, Some(2)),
    spec("list.take_last", 2, Some(2)),
    spec("list.drop_last", 2, Some(2)),
    spec("list.insert", 3, Some(3)),
    spec("list.remove", 2, Some(2)),
    spec("list.compact", 1, Some(1)),
    // -- string.* -----------------------------------------------------------
    spec("string.upper", 1, Some(1)),
    spec("string.lower", 1, Some(1)),
    spec("string.capitalize", 1, Some(2)),
    spec("string.case", 2, Some(2)),
    spec("string.replace", 3, Some(4)),
    spec("string.find", 2, Some(3)),
    spec("string.count", 2, Some(2)),
    spec("string.before", 2, Some(2)),
    spec("string.after", 2, Some(2)),
    spec("string.split", 2, Some(2)),
    spec("string.join", 2, Some(2)),
    spec("string.pad", 3, Some(4)),
    spec("string.pad_left", 2, Some(3)),
    spec("string.pad_right", 2, Some(3)),
    spec("string.repeat", 2, Some(2)),
    spec("string.slugify", 1, Some(1)),
    spec("string.escape", 2, Some(2)),
    spec("string.hex", 1, Some(1)),
    spec("string.char_at", 2, Some(2)),
    spec("string.code_at", 2, Some(2)),
    spec("string.regex_groups", 2, Some(3)),
    spec("string.matches", 2, Some(2)),
    spec("string.starts_with", 2, Some(2)),
    spec("string.ends_with", 2, Some(2)),
    spec("string.contains", 2, Some(2)),
    spec("string.words", 1, Some(1)),
    spec("string.is_blank", 1, Some(1)),
    spec("string.length", 1, Some(1)),
    spec("string.url_encode", 1, Some(1)),
    spec("string.url_decode", 1, Some(1)),
    spec("string.swap_case", 1, Some(1)),
    spec("string.trim", 1, Some(2)),
    spec("string.trim_left", 1, Some(1)),
    spec("string.trim_right", 1, Some(1)),
    spec("string.slice", 2, Some(3)),
    spec("string.prefix", 2, Some(2)),
    spec("string.suffix", 2, Some(2)),
    spec("string.reverse", 1, Some(1)),
    spec("string.normalize", 1, Some(2)),
    // -- text.* -------------------------------------------------------------
    spec("text.distance", 3, Some(3)),
    spec("text.similarity", 3, Some(3)),
    spec("text.phonetic", 2, Some(2)),
    spec("text.phonetic_match", 3, Some(3)),
    // -- map.* --------------------------------------------------------------
    spec("map.from", 1, Some(2)),
    spec("map.set", 3, Some(3)),
    spec("map.remove", 2, Some(2)),
    spec("map.merge", 2, Some(3)),
    spec("map.deep_merge", 2, Some(3)),
    spec("map.compact", 1, Some(1)),
    spec("map.group_by", 2, Some(2)),
    spec("map.flatten", 1, Some(2)),
    spec("map.unflatten", 1, Some(2)),
    spec("map.get_path", 2, Some(3)),
    spec("map.set_path", 3, Some(3)),
    spec("map.remove_path", 2, Some(2)),
    spec("map.entries", 1, Some(2)),
    spec("map.values", 1, Some(2)),
    spec("map.keys", 1, Some(1)),
    spec("map.has_key", 2, Some(2)),
    spec("map.pick", 2, Some(2)),
    spec("map.rename", 3, Some(3)),
    spec("map.invert", 1, Some(1)),
    spec("map.get", 2, Some(3)),
    spec("map.size", 1, Some(1)),
    spec("map.index_by", 2, Some(2)),
    // -- number.* -----------------------------------------------------------
    spec("number.format", 1, Some(3)),
    spec("number.to_base", 2, Some(2)),
    spec("number.from_base", 2, Some(2)),
    spec("number.to_roman", 1, Some(1)),
    spec("number.from_roman", 1, Some(1)),
    spec("bits.and", 2, Some(2)),
    spec("bits.or", 2, Some(2)),
    spec("bits.xor", 2, Some(2)),
    spec("bits.shift_left", 2, Some(2)),
    spec("bits.shift_right", 2, Some(2)),
    spec("bits.not", 1, Some(1)),
    spec("number.bitop", 3, Some(3)),
    spec("number.is_integer", 1, Some(1)),
    spec("number.is_even", 1, Some(1)),
    spec("number.is_odd", 1, Some(1)),
    spec("number.is_positive", 1, Some(1)),
    spec("number.is_negative", 1, Some(1)),
    spec("number.is_zero", 1, Some(1)),
    spec("number.is_nan", 1, Some(1)),
    spec("number.is_finite", 1, Some(1)),
    spec("number.is_infinite", 1, Some(1)),
    // -- math.* -------------------------------------------------------------
    spec("math.min", 1, None),
    spec("math.max", 1, None),
    spec("math.round", 1, Some(3)),
    spec("math.trunc", 1, Some(1)),
    spec("math.sigmoid", 1, Some(1)),
    spec("math.tanh", 1, Some(1)),
    spec("math.cosh", 1, Some(1)),
    spec("math.sinh", 1, Some(1)),
    spec("math.cot", 1, Some(1)),
    spec("math.coth", 1, Some(1)),
    spec("math.atan2", 2, Some(2)),
    spec("math.pow", 2, Some(2)),
    spec("math.hypot", 2, Some(2)),
    spec("math.log_base", 2, Some(2)),
    spec("math.gcd", 2, Some(2)),
    spec("math.lcm", 2, Some(2)),
    spec("math.clamp", 3, Some(3)),
    spec("math.lerp", 3, Some(3)),
    spec("math.abs", 1, Some(1)),
    spec("math.ceil", 1, Some(1)),
    spec("math.floor", 1, Some(1)),
    spec("math.sqrt", 1, Some(1)),
    spec("math.sign", 1, Some(1)),
    spec("math.log", 1, Some(1)),
    spec("math.ln", 1, Some(1)),
    spec("math.log10", 1, Some(1)),
    spec("math.exp", 1, Some(1)),
    spec("math.sin", 1, Some(1)),
    spec("math.cos", 1, Some(1)),
    spec("math.tan", 1, Some(1)),
    spec("math.asin", 1, Some(1)),
    spec("math.acos", 1, Some(1)),
    spec("math.atan", 1, Some(1)),
    spec("math.degrees", 1, Some(1)),
    spec("math.radians", 1, Some(1)),
    spec("math.pi", 0, Some(0)),
    spec("math.e", 0, Some(0)),
    spec("math.random", 0, Some(0)),
    // -- temporal.* ----------------------------------------------------------
    spec("temporal.now", 0, Some(1)),
    spec("temporal.today", 0, Some(0)),
    spec("temporal.timestamp", 0, Some(0)),
    spec("temporal.timezone", 0, Some(0)),
    spec("temporal.parse", 1, Some(3)),
    spec("temporal.format", 1, Some(2)),
    spec("temporal.reformat", 3, Some(3)),
    spec("temporal.convert", 3, Some(3)),
    spec("temporal.add", 2, Some(2)),
    spec("temporal.get", 2, Some(2)),
    spec("temporal.fields", 1, Some(1)),
    spec("temporal.truncate", 2, Some(2)),
    spec("temporal.between", 2, Some(2)),
    spec("temporal.in_days", 2, Some(2)),
    // -- bytes.* ------------------------------------------------------------
    spec("bytes.size", 1, Some(1)),
    spec("bytes.from_string", 1, Some(2)),
    spec("bytes.to_string", 1, Some(2)),
    spec("bytes.base64_encode", 1, Some(1)),
    spec("bytes.base64_decode", 1, Some(1)),
    spec("bytes.hex_encode", 1, Some(1)),
    spec("bytes.hex_decode", 1, Some(1)),
    spec("bytes.compress", 1, Some(2)),
    spec("bytes.decompress", 1, Some(2)),
    // -- crypto.* -----------------------------------------------------------
    spec("crypto.blake3", 1, Some(1)),
    spec("crypto.crc32", 1, Some(1)),
    // -- uuid.* -------------------------------------------------------------
    spec("uuid.new", 0, Some(0)),
    spec("uuid.from_string", 1, Some(1)),
    spec("uuid.is_valid", 1, Some(1)),
    // -- json.* -------------------------------------------------------------
    spec("json.encode", 1, Some(2)),
    spec("json.decode", 1, Some(1)),
    spec("json.path", 2, Some(2)),
    // -- geo.* --------------------------------------------------------------
    spec("geo.distance", 2, Some(2)),
    spec("geo.within_bbox", 3, Some(3)),
    spec("geo.point", 1, Some(1)),
    // -- vector.* -----------------------------------------------------------
    spec("vector.dimension", 1, Some(1)),
    spec_enum("vector.distance", 3, Some(3), &[2]),
    spec("vector.similarity", 2, Some(3)),
    spec_enum("vector.norm", 2, Some(2), &[1]),
    spec_enum("vector.coordinates", 2, Some(2), &[1]),
    // -- node.* -------------------------------------------------------------
    spec("node.id", 1, Some(1)),
    spec("node.labels", 1, Some(1)),
    spec("node.has_label", 2, Some(2)),
    spec("node.keys", 1, Some(1)),
    spec("node.properties", 1, Some(1)),
    // -- edge.* -------------------------------------------------------------
    spec("edge.id", 1, Some(1)),
    spec("edge.type", 1, Some(1)),
    spec("edge.keys", 1, Some(1)),
    spec("edge.properties", 1, Some(1)),
    spec("edge.start", 1, Some(1)),
    spec("edge.end", 1, Some(1)),
    // -- path.* -------------------------------------------------------------
    spec("path.nodes", 1, Some(1)),
    spec("path.edges", 1, Some(1)),
    spec("path.length", 1, Some(1)),
    spec("path.first", 1, Some(1)),
    spec("path.last", 1, Some(1)),
    // -- value.* (polymorphic) ----------------------------------------------
    spec("value.size", 1, Some(1)),
    spec("value.keys", 1, Some(1)),
    spec("value.properties", 1, Some(1)),
    spec("value.reverse", 1, Some(1)),
    spec("value.coalesce", 1, None),
    spec("value.is_null", 1, Some(1)),
    spec("value.is_not_null", 1, Some(1)),
    spec("value.id", 1, Some(1)),
    // -- type.* -------------------------------------------------------------
    spec("type.of", 1, Some(1)),
    spec_type("type.is", 2, Some(2), &[1]),
    // -- cast.* -------------------------------------------------------------
    spec_type("cast.to", 2, Some(2), &[1]),
    spec_type("cast.try", 2, Some(2), &[1]),
    spec_type("cast.can", 2, Some(2), &[1]),
];

pub const BUILTIN_ALIASES: &[BuiltinAlias] = &[
    // Lora migration aliases.
    alias("list.find_index", "list.index_of"),
    alias("list.find_indexes", "list.indexes_of"),
    alias("vector.dim", "vector.dimension"),
    alias("value.first_non_null", "value.coalesce"),
    alias("type.cast", "cast.to"),
    alias("type.try_cast", "cast.try"),
    alias("type.can_cast", "cast.can"),
    alias("now", "temporal.now"),
    alias("datetime", "temporal.now"),
    // Cypher temporal constructors. `temporal.now` doubles as a parser
    // when called with a string argument (see `temporal::now` in the
    // executor — `date("2025-01-01")` lands in the string-parse branch
    // that picks DateTime/Date/Duration based on the literal's shape).
    alias("date", "temporal.now"),
    alias("time", "temporal.now"),
    alias("localdatetime", "temporal.now"),
    alias("localtime", "temporal.now"),
    alias("duration", "temporal.now"),
    alias("point", "geo.point"),
    alias("timestamp", "temporal.timestamp"),
    alias("timezone", "temporal.timezone"),
    alias("new", "uuid.new"),
    alias("random", "math.random"),
    alias("rand", "math.random"),
    alias("range", "list.range"),
    // Cypher / historical compatibility aliases.
    alias("head", "list.first"),
    alias("last", "list.last"),
    alias("coalesce", "value.coalesce"),
    alias("tolower", "string.lower"),
    alias("toupper", "string.upper"),
    alias("left", "string.prefix"),
    alias("right", "string.suffix"),
    alias("substring", "string.slice"),
    alias("reverse", "value.reverse"),
    alias("size", "value.size"),
    alias("length", "path.length"),
    alias("keys", "value.keys"),
    alias("properties", "value.properties"),
    alias("id", "value.id"),
    alias("labels", "node.labels"),
    alias("type", "edge.type"),
    alias("randomuuid", "uuid.new"),
    alias("tostring", "cast.to"),
    alias("tointeger", "cast.to"),
    alias("tofloat", "cast.to"),
    alias("toboolean", "cast.to"),
    alias("tointegerornull", "cast.try"),
    alias("tofloatornull", "cast.try"),
    alias("tobooleanornull", "cast.try"),
    alias("tostringornull", "cast.try"),
];

pub fn builtin_spec(name: &str) -> Option<&'static BuiltinSpec> {
    canonical_builtin_name(name)
        .and_then(|canonical| BUILTIN_SPECS.iter().find(|spec| spec.name == canonical))
}

pub fn namespaced_arity(name: &str) -> Option<(usize, Option<usize>)> {
    builtin_spec(name).map(|spec| (spec.arity.min, spec.arity.max))
}

pub fn accepts_enum_literal(name: &str, arg_idx: usize) -> bool {
    builtin_spec(name).is_some_and(|spec| spec.enum_arg_slots.contains(&arg_idx))
}

pub fn accepts_type_literal(name: &str, arg_idx: usize) -> bool {
    builtin_spec(name).is_some_and(|spec| spec.type_arg_slots.contains(&arg_idx))
}

pub fn resolve_function(name: &str) -> Option<FunctionId> {
    let lower = name.to_ascii_lowercase();
    builtin_spec(&lower)
        .map(FunctionId::Builtin)
        .or_else(|| AggregateFunction::parse(&lower).map(FunctionId::Aggregate))
}

pub fn canonical_builtin_name(name: &str) -> Option<&'static str> {
    BUILTIN_SPECS
        .iter()
        .find(|spec| spec.name == name)
        .map(|spec| spec.name)
        .or_else(|| {
            BUILTIN_ALIASES
                .iter()
                .find(|alias| alias.alias == name)
                .map(|alias| alias.canonical)
        })
}
