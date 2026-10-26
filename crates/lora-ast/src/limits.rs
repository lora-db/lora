//! Hard limits on query shape and query-built values.
//!
//! Every stage of the engine (parser, analyzer, compiler, executor, the
//! value codecs) walks queries and values recursively. These limits keep
//! that recursion, and the memory a single expression can claim, bounded,
//! so a hostile or generated query fails with an error instead of
//! overflowing the stack (which aborts the whole process) or exhausting
//! memory.
//!
//! The numbers are sized so that a query at every limit at once still runs
//! on a 1 MiB thread stack in release builds, the smallest stack a host is
//! likely to call in on (WebAssembly's default, Ruby threads). The user
//! docs list them in `apps/loradb.com/docs/limits.md`.

/// Deepest a list or map value may nest, counting the outermost container
/// as depth 1. Parameters, query-built lists and maps, and the WAL /
/// snapshot / store codecs all enforce the same number, so anything a query
/// can build can be persisted and read back. Must equal
/// `lora_store::codec::MAX_VALUE_NESTING`.
pub const MAX_VALUE_NESTING: usize = 128;

/// Deepest that brackets, parentheses, braces and `CASE` expressions may
/// nest in query text. Checked by a linear scan before parsing.
pub const MAX_QUERY_NESTING: usize = 64;

/// Deepest an expression tree may grow through operator chains: each
/// operator in `a + b + c`, each `NOT`, each comparison, each unary sign and
/// each postfix lookup (`.key`, `[i]`, `::TYPE`) adds a level, summed along
/// the path from the outermost expression to the innermost one.
pub const MAX_EXPRESSION_DEPTH: usize = 256;

/// Most clauses one query branch may chain (`MATCH`, `WITH`, `CREATE`,
/// ...), counting the clauses of `CALL { ... }` bodies it contains. The
/// branches of a `UNION` are counted separately.
pub const MAX_CLAUSES: usize = 256;

/// Largest string, list, map or byte value a single expression may build,
/// and largest property value a write may store, in bytes (approximate for
/// lists and maps: a fixed cost per element plus the size of the strings
/// and bytes they hold). Kept well below the WAL's 256 MiB record limit so
/// any value a query can store also fits in one WAL record.
pub const MAX_VALUE_BYTES: usize = 128 << 20;

/// Largest hop count a variable-length relationship pattern (`-[*1..n]->`)
/// may name. A pattern without an upper bound (`-[*]->`, `-[*2..]->`)
/// stops at 100 hops.
pub const MAX_VAR_LENGTH_HOPS: u64 = 1_000;
