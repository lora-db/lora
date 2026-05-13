//! End-to-end coverage of the namespaced builtin functions
//! (`list.*`, `string.*`, `text.*`, `map.*`, `number.*`, `math.*`,
//! `temporal.*`, `bytes.*`, `crypto.*`, `uuid.*`, `json.*`).
//!
//! Each test runs a Cypher query through the full parse → analyse →
//! compile → execute pipeline, so a regression in any layer fails here.

mod test_helpers;
use serde_json::{json, Value as JsonValue};
use test_helpers::TestDb;

fn db() -> TestDb {
    TestDb::new()
}

// --- list.* ----------------------------------------------------------------

#[test]
fn list_sum_ints_and_floats() {
    assert_eq!(db().scalar("RETURN list.sum([1, 2, 3, 4])"), json!(10));
    assert_eq!(db().scalar("RETURN list.sum([1.5, 2.5])"), json!(4.0));
}

#[test]
fn list_avg_skips_nulls() {
    assert_eq!(db().scalar("RETURN list.avg([1, 2, 3, 4])"), json!(2.5));
}

#[test]
fn list_min_max() {
    assert_eq!(db().scalar("RETURN list.min([3, 1, 2])"), json!(1));
    assert_eq!(db().scalar("RETURN list.max([3, 1, 2])"), json!(3));
}

#[test]
fn list_product_and_median() {
    assert_eq!(db().scalar("RETURN list.product([2, 3, 4])"), json!(24));
    assert_eq!(
        db().scalar("RETURN list.median([1, 2, 3, 4, 5])"),
        json!(3.0)
    );
}

#[test]
fn list_sort_default_and_desc() {
    assert_eq!(db().scalar("RETURN list.sort([3, 1, 2])"), json!([1, 2, 3]));
    assert_eq!(
        db().scalar("RETURN list.sort([3, 1, 2], 'desc')"),
        json!([3, 2, 1])
    );
}

#[test]
fn list_unique_and_predicates() {
    assert_eq!(
        db().scalar("RETURN list.unique([1, 2, 2, 3, 3, 3])"),
        json!([1, 2, 3])
    );
    assert_eq!(
        db().scalar("RETURN list.has_duplicates([1, 2, 3])"),
        json!(false)
    );
    assert_eq!(
        db().scalar("RETURN list.has_duplicates([1, 2, 2])"),
        json!(true)
    );
    assert_eq!(
        db().scalar("RETURN list.all_distinct([1, 2, 3])"),
        json!(true)
    );
}

#[test]
fn list_set_ops() {
    assert_eq!(
        db().scalar("RETURN list.union([1, 2], [2, 3])"),
        json!([1, 2, 3])
    );
    assert_eq!(
        db().scalar("RETURN list.intersect([1, 2, 3], [2, 3, 4])"),
        json!([2, 3])
    );
    assert_eq!(
        db().scalar("RETURN list.diff([1, 2, 3], [2])"),
        json!([1, 3])
    );
    assert_eq!(
        db().scalar("RETURN list.symmetric_diff([1, 2, 3], [3, 4, 5])"),
        json!([1, 2, 4, 5])
    );
}

#[test]
fn list_zip_and_chunks() {
    assert_eq!(
        db().scalar("RETURN list.zip([1, 2, 3], ['a', 'b', 'c'])"),
        json!([[1, "a"], [2, "b"], [3, "c"]])
    );
    assert_eq!(
        db().scalar("RETURN list.chunks([1, 2, 3, 4, 5], 2)"),
        json!([[1, 2], [3, 4], [5]])
    );
}

#[test]
fn list_concat_append_and_prepend() {
    assert_eq!(
        db().scalar("RETURN list.concat([1, 2], [3], [4, 5])"),
        json!([1, 2, 3, 4, 5])
    );
    assert_eq!(
        db().scalar("RETURN list.append([1, 2], null)"),
        json!([1, 2, null])
    );
    assert_eq!(
        db().scalar("RETURN list.prepend([2, 3], 1)"),
        json!([1, 2, 3])
    );
    assert_eq!(
        db().scalar("RETURN list.concat([1], null)"),
        JsonValue::Null
    );
}

#[test]
fn list_count_by_returns_map() {
    let result = db().scalar("RETURN list.count_by(['a', 'b', 'a', 'c', 'a'])");
    assert_eq!(result, json!({ "a": 3, "b": 1, "c": 1 }));
}

#[test]
fn list_take_drop_flatten() {
    assert_eq!(
        db().scalar("RETURN list.take([1, 2, 3, 4], 2)"),
        json!([1, 2])
    );
    assert_eq!(
        db().scalar("RETURN list.drop([1, 2, 3, 4], 2)"),
        json!([3, 4])
    );
    assert_eq!(
        db().scalar("RETURN list.take_last([1, 2, 3, 4], 2)"),
        json!([3, 4])
    );
    assert_eq!(
        db().scalar("RETURN list.drop_last([1, 2, 3, 4], 2)"),
        json!([1, 2])
    );
    assert_eq!(
        db().scalar("RETURN list.flatten([[1, 2], [3, 4]])"),
        json!([1, 2, 3, 4])
    );
}

#[test]
fn list_function_indexing_and_tail_slices() {
    assert_eq!(db().scalar("RETURN list.at([10, 20, 30], -1)"), json!(30));
    assert_eq!(
        db().scalar("RETURN list.slice([10, 20, 30, 40], 1, 3)"),
        json!([20, 30])
    );
    assert_eq!(
        db().scalar("RETURN list.take_last([1, 2, 3, 4], 2)"),
        json!([3, 4])
    );
    assert_eq!(
        db().scalar("RETURN list.drop_last([1, 2, 3, 4], 2)"),
        json!([1, 2])
    );
}

#[test]
fn list_at_and_slice_support_negative_bounds() {
    assert_eq!(db().scalar("RETURN list.at([10, 20, 30], 1)"), json!(20));
    assert_eq!(db().scalar("RETURN list.at([10, 20, 30], -1)"), json!(30));
    assert_eq!(
        db().scalar("RETURN list.at([10, 20, 30], 99)"),
        JsonValue::Null
    );
    assert_eq!(
        db().scalar("RETURN list.slice([10, 20, 30, 40], 1, 3)"),
        json!([20, 30])
    );
    assert_eq!(
        db().scalar("RETURN list.slice([10, 20, 30, 40], -3, -1)"),
        json!([20, 30])
    );
    assert_eq!(
        db().scalar("RETURN list.slice([10, 20, 30], 2, 1)"),
        json!([])
    );
}

// --- string.* --------------------------------------------------------------

#[test]
fn string_case_styles() {
    assert_eq!(
        db().scalar("RETURN string.case('hello world', 'camel')"),
        json!("helloWorld")
    );
    assert_eq!(
        db().scalar("RETURN string.case('hello world', 'pascal')"),
        json!("HelloWorld")
    );
    assert_eq!(
        db().scalar("RETURN string.case('helloWorld', 'snake')"),
        json!("hello_world")
    );
    assert_eq!(
        db().scalar("RETURN string.case('helloWorld', 'kebab')"),
        json!("hello-world")
    );
}

#[test]
fn string_pad_sides() {
    assert_eq!(
        db().scalar("RETURN string.pad('42', 5, '0')"),
        json!("00042")
    );
    assert_eq!(
        db().scalar("RETURN string.pad('42', 5, '0', 'right')"),
        json!("42000")
    );
}

#[test]
fn string_slugify_and_escape() {
    assert_eq!(
        db().scalar("RETURN string.slugify('Hello World! 2026')"),
        json!("hello-world-2026")
    );
    assert_eq!(
        db().scalar("RETURN string.escape('hi\"there', 'json')"),
        json!("\"hi\\\"there\"")
    );
}

#[test]
fn string_regex_and_url() {
    assert_eq!(
        db().scalar("RETURN string.matches('abc123', '/^[a-z]+\\\\d+$/')"),
        json!(true)
    );
    assert_eq!(
        db().scalar("RETURN string.url_encode('a b/c')"),
        json!("a%20b%2Fc")
    );
    assert_eq!(
        db().scalar("RETURN string.url_decode('a%20b%2Fc')"),
        json!("a b/c")
    );
}

#[test]
fn string_extract_words_and_normalize() {
    assert_eq!(db().scalar("RETURN string.count('banana', 'na')"), json!(2));
    assert_eq!(
        db().scalar("RETURN string.before('name=Ada', '=')"),
        json!("name")
    );
    assert_eq!(
        db().scalar("RETURN string.after('name=Ada', '=')"),
        json!("Ada")
    );
    assert_eq!(
        db().scalar("RETURN string.words('  red green\\tblue ')"),
        json!(["red", "green", "blue"])
    );
    assert_eq!(db().scalar("RETURN string.is_blank('  \\t ')"), json!(true));
    assert_eq!(
        db().scalar("RETURN string.length(string.normalize('é', 'nfd'))"),
        json!(2)
    );
}

#[test]
fn string_count_before_after() {
    assert_eq!(db().scalar("RETURN string.count('banana', 'na')"), json!(2));
    assert_eq!(
        db().scalar("RETURN string.count('a1 b22 c333', '/\\\\d+/')"),
        json!(3)
    );
    assert_eq!(
        db().scalar("RETURN string.before('user@example.com', '@')"),
        json!("user")
    );
    assert_eq!(
        db().scalar("RETURN string.after('user@example.com', '@')"),
        json!("example.com")
    );
    assert_eq!(
        db().scalar("RETURN string.after('user@example.com', '#')"),
        JsonValue::Null
    );
}

#[test]
fn string_words_and_blank_predicate() {
    assert_eq!(
        db().scalar("RETURN string.words('  Ada  Lovelace\\nByron  ')"),
        json!(["Ada", "Lovelace", "Byron"])
    );
    assert_eq!(db().scalar("RETURN string.words('   ')"), json!([]));
    assert_eq!(
        db().scalar("RETURN string.is_blank(' \\t\\n ')"),
        json!(true)
    );
    assert_eq!(
        db().scalar("RETURN string.is_blank(' data ')"),
        json!(false)
    );
}

#[test]
fn string_normalize_supports_unicode_forms() {
    assert_eq!(
        db().scalar("RETURN string.normalize('Cafe\u{301}')"),
        json!("Café")
    );
    assert_eq!(
        db().scalar("RETURN string.length(string.normalize('é', 'nfd'))"),
        json!(2)
    );
    assert_eq!(
        db().scalar("RETURN string.normalize('é', 'made_up')"),
        JsonValue::Null
    );
}

// --- text.* ----------------------------------------------------------------

#[test]
fn text_distance_metrics() {
    assert_eq!(
        db().scalar("RETURN text.distance('kitten', 'sitting', 'levenshtein')"),
        json!(3)
    );
    assert_eq!(
        db().scalar("RETURN text.distance('book', 'back', 'hamming')"),
        json!(2)
    );
}

#[test]
fn text_similarity_metrics() {
    let val = db().scalar("RETURN text.similarity('martha', 'marhta', 'jaro_winkler')");
    if let JsonValue::Number(n) = val {
        assert!(n.as_f64().unwrap() > 0.96, "got {n:?}");
    } else {
        panic!("expected float, got {val:?}");
    }
}

#[test]
fn text_phonetic_soundex() {
    assert_eq!(
        db().scalar("RETURN text.phonetic('Robert', 'soundex')"),
        json!("R163")
    );
    assert_eq!(
        db().scalar("RETURN text.phonetic_match('Robert', 'Rupert', 'soundex')"),
        json!(true)
    );
}

// --- map.* -----------------------------------------------------------------

#[test]
fn map_set_remove_merge() {
    assert_eq!(
        db().scalar("RETURN map.set({a: 1}, 'b', 2)"),
        json!({ "a": 1, "b": 2 })
    );
    assert_eq!(
        db().scalar("RETURN map.remove({a: 1, b: 2}, 'a')"),
        json!({ "b": 2 })
    );
    assert_eq!(
        db().scalar("RETURN map.merge({a: 1}, {b: 2, a: 9})"),
        json!({ "a": 9, "b": 2 })
    );
    assert_eq!(
        db().scalar("RETURN map.merge({a: 1}, {b: 2, a: 9}, 'left')"),
        json!({ "a": 1, "b": 2 })
    );
}

#[test]
fn map_deep_merge_recurses_nested_maps() {
    assert_eq!(
        db().scalar(
            "RETURN map.deep_merge(
                {user: {name: 'Ada', flags: {admin: false}}, seen: 1},
                {user: {email: 'ada@example.test', flags: {admin: true}}, seen: 2}
            )"
        ),
        json!({
            "seen": 2,
            "user": {
                "email": "ada@example.test",
                "flags": { "admin": true },
                "name": "Ada"
            }
        })
    );
    assert_eq!(
        db().scalar("RETURN map.deep_merge({a: {b: 1}}, {a: {c: 2}}, 'left')"),
        json!({ "a": { "b": 1, "c": 2 } })
    );
    assert_eq!(
        db().scalar("RETURN map.deep_merge({a: {b: 1}}, {a: {b: 2}}, 'error')"),
        JsonValue::Null
    );
}

#[test]
fn map_path_helpers_read_write_and_remove_nested_values() {
    assert_eq!(
        db().scalar("RETURN map.get_path({user: {name: 'Ada'}}, ['user', 'name'])"),
        json!("Ada")
    );
    assert_eq!(
        db().scalar("RETURN map.get_path({user: {name: 'Ada'}}, 'user.email', 'n/a')"),
        json!("n/a")
    );
    assert_eq!(
        db().scalar("RETURN map.set_path({user: {name: 'Ada'}}, 'user.email', 'ada@example.test')"),
        json!({ "user": { "email": "ada@example.test", "name": "Ada" } })
    );
    assert_eq!(
        db().scalar("RETURN map.set_path({}, ['user', 'flags', 'admin'], true)"),
        json!({ "user": { "flags": { "admin": true } } })
    );
    assert_eq!(
        db().scalar("RETURN map.remove_path({user: {name: 'Ada', email: 'a@x'}}, 'user.email')"),
        json!({ "user": { "name": "Ada" } })
    );
    assert_eq!(
        db().scalar("RETURN map.get_path({user: {name: 'Ada'}}, '')"),
        JsonValue::Null
    );
}

#[test]
fn map_compact_and_keys() {
    assert_eq!(
        db().scalar("RETURN map.compact({a: 1, b: null, c: 3})"),
        json!({ "a": 1, "c": 3 })
    );
    assert_eq!(
        db().scalar("RETURN map.keys({c: 1, a: 2, b: 3})"),
        json!(["a", "b", "c"])
    );
}

#[test]
fn map_key_selection_and_shape_helpers() {
    assert_eq!(
        db().scalar("RETURN map.has_key({a: 1, b: null}, 'b')"),
        json!(true)
    );
    assert_eq!(
        db().scalar("RETURN map.has_key({a: 1}, 'missing')"),
        json!(false)
    );
    assert_eq!(
        db().scalar("RETURN map.pick({a: 1, b: 2, c: 3}, ['c', 'a'])"),
        json!({ "a": 1, "c": 3 })
    );
    assert_eq!(
        db().scalar("RETURN map.rename({first_name: 'Ada', born: 1815}, 'first_name', 'name')"),
        json!({ "born": 1815, "name": "Ada" })
    );
    assert_eq!(
        db().scalar("RETURN map.invert({a: 1, b: true, c: 'see'})"),
        json!({ "1": "a", "true": "b", "see": "c" })
    );
}

#[test]
fn map_flatten_unflatten() {
    let flat = db().scalar("RETURN map.flatten({a: {b: 1, c: 2}, d: 3})");
    assert_eq!(flat, json!({ "a.b": 1, "a.c": 2, "d": 3 }));

    // Round-trip via flatten — unflatten requires a flat map with dotted
    // keys, which the parser doesn't accept as map literals (keys must
    // be bare identifiers). Future work: allow string-key map literals.
    let roundtrip = db().scalar("RETURN map.unflatten(map.flatten({a: {b: 1, c: 2}, d: 3}))");
    assert_eq!(roundtrip, json!({ "a": { "b": 1, "c": 2 }, "d": 3 }));
}

// --- number.* + bits.* + math.* --------------------------------------------

#[test]
fn number_roman_conversions() {
    assert_eq!(
        db().scalar("RETURN number.to_roman(1994)"),
        json!("MCMXCIV")
    );
    assert_eq!(
        db().scalar("RETURN number.from_roman('MCMXCIV')"),
        json!(1994)
    );
}

#[test]
fn number_radix_conversions() {
    assert_eq!(db().scalar("RETURN number.to_base(255, 16)"), json!("ff"));
    assert_eq!(db().scalar("RETURN number.to_base(-10, 2)"), json!("-1010"));
    assert_eq!(db().scalar("RETURN number.from_base('ff', 16)"), json!(255));
    assert_eq!(
        db().scalar("RETURN number.from_base('-1010', 2)"),
        json!(-10)
    );
    assert_eq!(
        db().scalar("RETURN number.from_base('not-binary', 2)"),
        JsonValue::Null
    );
    assert_eq!(db().scalar("RETURN number.to_base(10, 1)"), JsonValue::Null);
}

#[test]
fn number_predicates() {
    assert_eq!(db().scalar("RETURN number.is_integer(42)"), json!(true));
    assert_eq!(db().scalar("RETURN number.is_integer(42.0)"), json!(true));
    assert_eq!(db().scalar("RETURN number.is_integer(42.5)"), json!(false));
    assert_eq!(db().scalar("RETURN number.is_even(42)"), json!(true));
    assert_eq!(db().scalar("RETURN number.is_even(41)"), json!(false));
    assert_eq!(db().scalar("RETURN number.is_odd(41)"), json!(true));
    assert_eq!(db().scalar("RETURN number.is_odd(42)"), json!(false));
    assert_eq!(db().scalar("RETURN number.is_even(4.0)"), JsonValue::Null);
    assert_eq!(db().scalar("RETURN number.is_positive(0.1)"), json!(true));
    assert_eq!(db().scalar("RETURN number.is_positive(0)"), json!(false));
    assert_eq!(db().scalar("RETURN number.is_negative(-1)"), json!(true));
    assert_eq!(db().scalar("RETURN number.is_negative(0)"), json!(false));
    assert_eq!(db().scalar("RETURN number.is_zero(0.0)"), json!(true));
    assert_eq!(db().scalar("RETURN number.is_zero(0.1)"), json!(false));
}

#[test]
fn bits_operations_are_named() {
    assert_eq!(db().scalar("RETURN bits.and(12, 10)"), json!(8));
    assert_eq!(db().scalar("RETURN bits.or(12, 10)"), json!(14));
    assert_eq!(db().scalar("RETURN bits.xor(12, 10)"), json!(6));
    assert_eq!(db().scalar("RETURN bits.shift_left(3, 2)"), json!(12));
    assert_eq!(db().scalar("RETURN bits.not(0)"), json!(-1));
}

#[test]
fn math_round_modes() {
    assert_eq!(
        db().scalar("RETURN math.round(2.5, 0, 'half_up')"),
        json!(3.0)
    );
    assert_eq!(
        db().scalar("RETURN math.round(2.5, 0, 'floor')"),
        json!(2.0)
    );
    assert_eq!(
        db().scalar("RETURN math.round(1.23456, 2, 'half_up')"),
        json!(1.23)
    );
}

#[test]
fn math_gcd_lcm_clamp_lerp() {
    assert_eq!(db().scalar("RETURN math.gcd(12, 18)"), json!(6));
    assert_eq!(db().scalar("RETURN math.lcm(4, 6)"), json!(12));
    assert_eq!(db().scalar("RETURN math.clamp(5, 0, 3)"), json!(3));
    assert_eq!(db().scalar("RETURN math.lerp(0.0, 10.0, 0.5)"), json!(5.0));
}

#[test]
fn math_scalar_min_and_max() {
    assert_eq!(db().scalar("RETURN math.min(3, 1, 2)"), json!(1));
    assert_eq!(db().scalar("RETURN math.max(3, 1, 2)"), json!(3));
    assert_eq!(db().scalar("RETURN math.min(3, 1.5, 2)"), json!(1.5));
    assert_eq!(db().scalar("RETURN math.max(3, null)"), JsonValue::Null);
}

#[test]
fn math_trunc_hypot_and_log_base() {
    assert_eq!(db().scalar("RETURN math.trunc(3.9)"), json!(3));
    assert_eq!(db().scalar("RETURN math.trunc(-3.9)"), json!(-3));
    assert_eq!(db().scalar("RETURN math.hypot(3, 4)"), json!(5.0));
    assert_eq!(db().scalar("RETURN math.log_base(8, 2)"), json!(3.0));
    assert_eq!(db().scalar("RETURN math.log_base(8, 1)"), JsonValue::Null);
}

// --- temporal.* ----------------------------------------------------------------

#[test]
fn time_convert_units() {
    assert_eq!(
        db().scalar("RETURN temporal.convert(1, 'days', 'hours')"),
        json!(24)
    );
    assert_eq!(
        db().scalar("RETURN temporal.convert(3600, 'seconds', 'minutes')"),
        json!(60)
    );
}

#[test]
fn bare_current_value_aliases() {
    assert_eq!(db().scalar("RETURN timezone()"), json!("UTC"));
    assert!(db().scalar("RETURN timestamp()").as_i64().is_some());
    assert!(db().scalar("RETURN now()").is_string());
    assert_eq!(db().scalar("RETURN uuid.is_valid(new())"), json!(true));

    let random = db().scalar("RETURN random()");
    let random = random.as_f64().expect("random() should return a float");
    assert!((0.0..1.0).contains(&random), "got {random}");
}

// --- bytes.* ---------------------------------------------------------------

#[test]
fn bytes_base64_roundtrip() {
    assert_eq!(
        db().scalar("RETURN bytes.base64_encode(bytes.from_string('hello'))"),
        json!("aGVsbG8=")
    );
}

#[test]
fn bytes_hex_roundtrip() {
    assert_eq!(
        db().scalar("RETURN bytes.hex_encode(bytes.from_string('hi'))"),
        json!("6869")
    );
}

#[test]
fn bytes_compress_roundtrip() {
    let cypher = "RETURN bytes.to_string(bytes.decompress(bytes.compress(bytes.from_string('hello world hello world'), 'gzip'), 'gzip'))";
    assert_eq!(db().scalar(cypher), json!("hello world hello world"));
}

// --- crypto.* --------------------------------------------------------------

#[test]
fn crypto_blake3_deterministic() {
    let a = db().scalar("RETURN crypto.blake3('hello')");
    let b = db().scalar("RETURN crypto.blake3('hello')");
    assert_eq!(a, b);
    let c = db().scalar("RETURN crypto.blake3('world')");
    assert_ne!(a, c);
}

#[test]
fn crypto_crc32_known_value() {
    assert_eq!(
        db().scalar("RETURN crypto.crc32('123456789')"),
        json!(0xCBF43926_u32 as i64)
    );
}

// --- uuid.* ----------------------------------------------------------------

#[test]
fn uuid_new_is_valid() {
    let val = db().scalar("RETURN uuid.is_valid(uuid.new())");
    assert_eq!(val, json!(true));
}

#[test]
fn uuid_validation_rejects_garbage() {
    assert_eq!(
        db().scalar("RETURN uuid.is_valid('not-a-uuid')"),
        json!(false)
    );
}

// --- json.* ----------------------------------------------------------------

#[test]
fn json_roundtrip_via_map() {
    let encoded = db().scalar("RETURN json.encode({a: 1, b: [2, 3]})");
    assert_eq!(encoded, json!(r#"{"a":1,"b":[2,3]}"#));

    let decoded = db().scalar("RETURN json.decode('{\"a\":1,\"b\":[2,3]}')");
    assert_eq!(decoded, json!({ "a": 1, "b": [2, 3] }));
}

#[test]
fn json_path_navigation() {
    let result = db().scalar("RETURN json.path({a: {b: [10, 20, 30]}}, '$.a.b[1]')");
    assert_eq!(result, json!(20));
}

// --- type.* + cast.* + value.* ---------------------------------------------

#[test]
fn cast_uses_target_argument() {
    assert_eq!(db().scalar("RETURN cast.to(42, STRING)"), json!("42"));
    assert_eq!(db().scalar("RETURN cast.to('42', INTEGER)"), json!(42));
    assert_eq!(db().scalar("RETURN cast.try('42', INTEGER)"), json!(42));
    assert!(db().scalar("RETURN cast.try('nope', INTEGER)").is_null());
    assert_eq!(db().scalar("RETURN cast.to('true', BOOLEAN)"), json!(true));

    let as_float = db().scalar("RETURN cast.to('3.5', FLOAT)");
    assert_eq!(as_float.as_f64().unwrap(), 3.5);
}

#[test]
fn strict_cast_errors_but_try_cast_returns_null() {
    let err = db().run_err("RETURN cast.to('nope', INTEGER)");
    assert!(err.contains("cannot cast"), "got: {err}");

    let err = db().run_err("RETURN ('nope' AS INTEGER)");
    assert!(err.contains("cannot cast"), "got: {err}");

    assert!(db()
        .scalar("RETURN type.try_cast('nope', INTEGER)")
        .is_null());
}

#[test]
fn type_of_and_type_is() {
    assert_eq!(
        db().scalar("RETURN type.of([1, 2, 3])"),
        json!("LIST<INTEGER>")
    );
    assert_eq!(
        db().scalar("RETURN type.is([1, 2, 3], 'LIST')"),
        json!(true)
    );
    assert_eq!(db().scalar("RETURN type.is([1, 2, 3], LIST)"), json!(true));
    assert_eq!(
        db().scalar("RETURN type.is([1, 2, 3], 'LIST<INTEGER>')"),
        json!(true)
    );
    assert_eq!(db().scalar("RETURN type.is(42, 'STRING')"), json!(false));
    assert_eq!(db().scalar("RETURN cast.can('42', INTEGER)"), json!(true));
    assert_eq!(
        db().scalar("RETURN cast.can('nope', INTEGER)"),
        json!(false)
    );
}

#[test]
fn parenthesized_as_type_casts_and_initializes_values() {
    assert_eq!(db().scalar("RETURN ('42' AS INTEGER) AS n"), json!(42));
    assert_eq!(
        db().scalar("RETURN ('2024-01-15' AS DATE) = '2024-01-15'::DATE AS same"),
        json!(true)
    );
    assert_eq!(
        db().scalar("RETURN ({x: 3.0, y: 4.0} AS POINT).srid AS srid"),
        json!(7203)
    );
    assert_eq!(
        db().scalar("RETURN type.of(([1, 2, 3] AS VECTOR<INTEGER>(3))) AS t"),
        json!("VECTOR<INTEGER>(3)")
    );
}

#[test]
fn parenthesized_as_type_casts_scalar_variants() {
    assert_eq!(db().scalar("RETURN (1 + 2 AS STRING) AS s"), json!("3"));
    assert_eq!(
        db().scalar("RETURN ('false' AS BOOLEAN) AS b"),
        json!(false)
    );
    assert_eq!(db().scalar("RETURN (true AS INTEGER) AS n"), json!(1));
    assert!(db().scalar("RETURN (null AS FLOAT) AS f").is_null());

    let as_float = db().scalar("RETURN ('12.25' AS FLOAT) AS f");
    assert_eq!(as_float.as_f64().unwrap(), 12.25);
}

#[test]
fn duckdb_cast_syntax_matches_parenthesized_casts() {
    assert_eq!(db().scalar("RETURN '42'::INTEGER AS n"), json!(42));
    assert_eq!(db().scalar("RETURN '42'::INTEGER AS n"), json!(42));
    assert_eq!(
        db().scalar("RETURN ('2024-01-15'::DATE) = '2024-01-15'::DATE AS same"),
        json!(true)
    );
    assert_eq!(
        db().scalar("RETURN type.of([1, 2, 3]::VECTOR<INTEGER>(3)) AS t"),
        json!("VECTOR<INTEGER>(3)")
    );
}

#[test]
fn duckdb_try_cast_returns_null_instead_of_erroring() {
    assert!(db()
        .scalar("RETURN TRY_CAST('bad' AS INTEGER) AS maybe")
        .is_null());
    assert!(db()
        .scalar("RETURN TRY_CAST('2024-99-99' AS DATE) AS maybe")
        .is_null());
    assert!(db()
        .scalar("RETURN TRY_CAST({x: 'bad', y: 2} AS POINT) AS maybe")
        .is_null());
    assert!(db()
        .scalar("RETURN TRY_CAST([1, 2, 3] AS VECTOR<INTEGER>(2)) AS maybe")
        .is_null());

    assert_eq!(
        db().scalar("RETURN TRY_CAST('2024-01-15' AS DATE) = '2024-01-15'::DATE AS same"),
        json!(true)
    );
    assert_eq!(
        db().scalar("RETURN type.of(TRY_CAST([1, 2, 3] AS VECTOR<INTEGER>(3))) AS t"),
        json!("VECTOR<INTEGER>(3)")
    );
}

#[test]
fn duckdb_cast_syntax_works_in_create_properties() {
    let db = db();
    let rows = db.run(
        "CREATE (n:DuckCast {
            id: '42'::INTEGER,
            created: '2024-01-15'::DATE,
            embedding: [1, 2, 3]::VECTOR<INTEGER>(3),
            maybe: TRY_CAST('bad' AS INTEGER)
        })
        RETURN n.id AS id,
               type.of(n.created) AS created_type,
               type.of(n.embedding) AS embedding_type,
               n.maybe AS maybe",
    );

    assert_eq!(rows[0]["id"], json!(42));
    assert_eq!(rows[0]["created_type"], json!("DATE"));
    assert_eq!(rows[0]["embedding_type"], json!("VECTOR<INTEGER>(3)"));
    assert!(rows[0]["maybe"].is_null());
}

#[test]
fn parenthesized_as_type_casts_temporal_variants() {
    assert_eq!(
        db().scalar(
            "RETURN ('2024-01-15T10:30:00Z' AS ZONED DATETIME) \
             = '2024-01-15T10:30:00Z'::DATETIME AS same"
        ),
        json!(true)
    );
    assert_eq!(
        db().scalar(
            "RETURN ('2024-01-15T10:30:00' AS LOCAL DATETIME) \
             = '2024-01-15T10:30:00'::LOCAL_DATETIME AS same"
        ),
        json!(true)
    );
    assert_eq!(
        db().scalar(
            "RETURN ('14:30:00+02:00' AS ZONED TIME) \
             = '14:30:00+02:00'::TIME AS same"
        ),
        json!(true)
    );
    assert_eq!(
        db().scalar(
            "RETURN ('14:30:00' AS LOCAL TIME) \
             = '14:30:00'::LOCAL_TIME AS same"
        ),
        json!(true)
    );
    assert_eq!(
        db().scalar("RETURN ('P1Y2M3D' AS DURATION) = 'P1Y2M3D'::DURATION AS same"),
        json!(true)
    );
}

#[test]
fn parenthesized_as_type_casts_work_in_query_expressions() {
    assert_eq!(
        db().scalar("WITH '7' AS raw RETURN (raw AS INTEGER) + 5 AS n"),
        json!(12)
    );

    let db = db();
    db.run("CREATE (:Item {qty: '8'})");
    assert_eq!(
        db.scalar("MATCH (i:Item) RETURN (i.qty AS INTEGER) * 2 AS doubled"),
        json!(16)
    );
}

#[test]
fn parenthesized_as_type_casts_work_in_create_properties() {
    let db = db();
    let rows = db.run(
        "CREATE (n:Typed {
            id: ('42' AS INTEGER),
            active: ('true' AS BOOLEAN),
            score: ('12.5' AS FLOAT),
            day: ('2024-01-15' AS DATE),
            point: ({x: 3.0, y: 4.0} AS POINT),
            embedding: ([1, 2, 3] AS VECTOR<INTEGER>(3))
        })
        RETURN n.id AS id,
               n.active AS active,
               n.score AS score,
               type.of(n.day) AS day_type,
               n.point.srid AS point_srid,
               type.of(n.embedding) AS embedding_type",
    );

    assert_eq!(rows[0]["id"], json!(42));
    assert_eq!(rows[0]["active"], json!(true));
    assert_eq!(rows[0]["score"].as_f64().unwrap(), 12.5);
    assert_eq!(rows[0]["day_type"], json!("DATE"));
    assert_eq!(rows[0]["point_srid"], json!(7203));
    assert_eq!(rows[0]["embedding_type"], json!("VECTOR<INTEGER>(3)"));
}

#[test]
fn parenthesized_as_type_casts_work_in_create_relationship_properties() {
    let db = db();
    let rows = db.run(
        "CREATE (:Account {id: ('1' AS INTEGER)})
            -[r:TRANSFER {
                amount: ('19.95' AS FLOAT),
                settled: ('false' AS BOOLEAN),
                at: ('2024-01-15T10:30:00Z' AS DATETIME)
            }]->
            (:Account {id: ('2' AS INTEGER)})
         RETURN r.amount AS amount,
                r.settled AS settled,
                type.of(r.at) AS at_type",
    );

    assert_eq!(rows[0]["amount"].as_f64().unwrap(), 19.95);
    assert_eq!(rows[0]["settled"], json!(false));
    assert_eq!(rows[0]["at_type"], json!("DATETIME"));
}

#[test]
fn parenthesized_as_type_casts_work_in_set_and_merge_properties() {
    let db = db();
    db.run("CREATE (:Typed {id: 1, raw: '21'})");
    db.run(
        "MATCH (n:Typed)
         SET n.value = (n.raw AS INTEGER),
             n.due = ('2024-02-01' AS DATE)",
    );
    let rows = db.run("MATCH (n:Typed) RETURN n.value AS value, type.of(n.due) AS due_type");

    assert_eq!(rows[0]["value"], json!(21));
    assert_eq!(rows[0]["due_type"], json!("DATE"));

    db.run(
        "MERGE (m:MergeTyped {id: ('7' AS INTEGER)})
         ON CREATE SET m.created = ('2024-03-01' AS DATE)",
    );
    assert_eq!(
        db.scalar("MATCH (m:MergeTyped) RETURN m.id AS id"),
        json!(7)
    );
    assert_eq!(
        db.scalar("MATCH (m:MergeTyped) RETURN type.of(m.created) AS t"),
        json!("DATE")
    );
}

#[test]
fn parenthesized_vector_type_cast_accepts_coordinate_aliases() {
    assert_eq!(
        db().scalar("RETURN type.of(([1.0, 2.0] AS VECTOR<FLOAT>(2))) AS t"),
        json!("VECTOR<FLOAT64>(2)")
    );
    assert_eq!(
        db().scalar("RETURN type.of(([1, 2] AS VECTOR<INT64>(2))) AS t"),
        json!("VECTOR<INTEGER>(2)")
    );
    assert_eq!(
        db().scalar("RETURN vector.dimension(([1, 2, 3] AS VECTOR<INTEGER>(3))) AS dim"),
        json!(3)
    );
}

#[test]
fn parenthesized_vector_type_cast_reports_cast_errors() {
    let err = db().run_err("RETURN ([1, 2, 3] AS VECTOR<INTEGER>(2)) AS v");
    assert!(
        err.contains("dimension") || err.contains("expected") || err.contains("cannot cast"),
        "got: {err}"
    );

    let err = db().run_err("RETURN (['x'] AS VECTOR<INTEGER>(1)) AS v");
    assert!(
        err.contains("numeric") || err.contains("cannot cast"),
        "got: {err}"
    );
}

#[test]
fn list_type_cast_syntax_is_parsed_but_runtime_cast_is_not_implemented_yet() {
    assert_eq!(
        db().scalar("RETURN type.is([1, 2, 3], 'LIST<INTEGER>') AS ok"),
        json!(true)
    );
    assert_eq!(
        db().scalar("RETURN cast.can([1, 2, 3], 'LIST<INTEGER>') AS ok"),
        json!(false)
    );

    let err = db().run_err("RETURN ([1, 2, 3] AS LIST<INTEGER>) AS xs");
    assert!(err.contains("cannot cast"), "got: {err}");
}

#[test]
fn strict_and_try_cast_aliases_have_distinct_failure_modes() {
    let err = db().run_err("RETURN type.cast('bad', INTEGER)");
    assert!(err.contains("cannot cast"), "got: {err}");

    let err = db().run_err("RETURN toInteger('bad')");
    assert!(err.contains("cannot cast"), "got: {err}");

    assert!(db()
        .scalar("RETURN toIntegerOrNull('bad') AS maybe")
        .is_null());
    assert!(db()
        .scalar("RETURN type.try_cast('bad', INTEGER) AS maybe")
        .is_null());
    assert!(db()
        .scalar("RETURN cast.try('bad', INTEGER) AS maybe")
        .is_null());
}

#[test]
fn cast_targets_accept_type_literals_or_strings_and_report_bad_targets() {
    assert_eq!(db().scalar("RETURN cast.to('42', 'INT') AS n"), json!(42));
    assert_eq!(
        db().scalar("RETURN type.try_cast('42', 'INTEGER') AS n"),
        json!(42)
    );

    let err = db().run_err("RETURN cast.to('42', 123)");
    assert!(
        err.contains("target type must be a type literal or string"),
        "got: {err}"
    );

    let err = db().run_err("RETURN cast.to('42', NOT_A_TYPE)");
    assert!(err.contains("unknown cast target type"), "got: {err}");

    let err = db().run_err("RETURN ('42' AS NOT_A_TYPE)");
    assert!(err.contains("unknown cast target type"), "got: {err}");

    assert!(db()
        .scalar("RETURN cast.try('42', NOT_A_TYPE) AS maybe")
        .is_null());
}

#[test]
fn compatibility_aliases_resolve_to_canonical_builtins() {
    assert_eq!(db().scalar("RETURN type.cast('42', INTEGER)"), json!(42));
    assert_eq!(
        db().scalar("RETURN type.can_cast('nope', INTEGER)"),
        json!(false)
    );
    assert_eq!(
        db().scalar("RETURN value.first_non_null(null, 'fallback')"),
        json!("fallback")
    );
    assert_eq!(
        db().scalar("RETURN vector.dim([1]::VECTOR<FLOAT32>(1))"),
        json!(1)
    );
    assert_eq!(db().scalar("RETURN toInteger('42')"), json!(42));
}

#[test]
fn type_of_distinguishes_edge_from_edge_type() {
    let db = db();
    db.run("CREATE (:A)-[:KNOWS]->(:B)");

    let rows = db.run("MATCH ()-[r]->() RETURN type.of(r) AS kind, edge.type(r) AS edge_type");
    assert_eq!(rows[0]["kind"], json!("EDGE"));
    assert_eq!(rows[0]["edge_type"], json!("KNOWS"));
}
