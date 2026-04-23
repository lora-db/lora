package lora

// This file contains the typed constructors and type guards that
// mirror the `shared-ts/types.ts` helpers used by the JS/TS bindings.
// Keeping the factory functions small means users never type out the
// `"kind": "…"` string literally, and the guards read cleanly at the
// call site.

// ---------------------------------------------------------------------------
// SRID constants — canonical IDs for the supported spatial reference systems.
// ---------------------------------------------------------------------------

const (
	SRIDCartesian2D int64 = 7203
	SRIDCartesian3D int64 = 9157
	SRIDWGS84_2D    int64 = 4326
	SRIDWGS84_3D    int64 = 4979
)

// ---------------------------------------------------------------------------
// Temporal constructors
//
// Each returns a tagged map with `{"kind": "<type>", "iso": "<iso>"}`.
// The ISO string is validated by the engine when the query runs, not
// by the helper itself — passing an invalid ISO surfaces as a
// LoraError with Code == CodeInvalidParams at execute time.
// ---------------------------------------------------------------------------

// Date returns a tagged date value from a `YYYY-MM-DD` string.
func Date(iso string) map[string]any { return map[string]any{"kind": "date", "iso": iso} }

// Time returns a tagged time value from a `HH:MM:SS[.nnn][Z|±HH:MM]` string.
func Time(iso string) map[string]any { return map[string]any{"kind": "time", "iso": iso} }

// LocalTime returns a tagged zone-less time from a `HH:MM:SS[.nnn]` string.
func LocalTime(iso string) map[string]any {
	return map[string]any{"kind": "localtime", "iso": iso}
}

// DateTime returns a tagged zoned datetime from an ISO-8601 string.
func DateTime(iso string) map[string]any {
	return map[string]any{"kind": "datetime", "iso": iso}
}

// LocalDateTime returns a tagged zone-less datetime from an ISO-8601 string.
func LocalDateTime(iso string) map[string]any {
	return map[string]any{"kind": "localdatetime", "iso": iso}
}

// Duration returns a tagged ISO-8601 duration value (e.g. `P1Y2M3DT4H5M6S`).
func Duration(iso string) map[string]any {
	return map[string]any{"kind": "duration", "iso": iso}
}

// ---------------------------------------------------------------------------
// Vector
// ---------------------------------------------------------------------------

// Canonical coordinate-type tags emitted by the engine. The `vector()`
// constructor in Cypher also accepts aliases (FLOAT, INT, SIGNED INTEGER,
// …) but every VECTOR returned to Go surfaces one of these six strings.
const (
	VectorCoordTypeFloat64   = "FLOAT64"
	VectorCoordTypeFloat32   = "FLOAT32"
	VectorCoordTypeInteger   = "INTEGER"
	VectorCoordTypeInteger32 = "INTEGER32"
	VectorCoordTypeInteger16 = "INTEGER16"
	VectorCoordTypeInteger8  = "INTEGER8"
)

// Vector builds a VECTOR parameter value. Values may be `[]float64`,
// `[]float32`, `[]int64`, `[]int`, `[]int32`, `[]int16`, `[]int8`, or
// a pre-built `[]any` of numbers — whatever fits the call site. The
// JSON bridge accepts any numeric-looking list element.
func Vector(values []any, dimension int, coordinateType string) map[string]any {
	return map[string]any{
		"kind":           "vector",
		"dimension":      dimension,
		"coordinateType": coordinateType,
		"values":         values,
	}
}

// IsVector reports whether v is a VECTOR value.
func IsVector(v any) bool { return kindOf(v) == "vector" }

// ---------------------------------------------------------------------------
// Spatial constructors
// ---------------------------------------------------------------------------

// Cartesian builds a Cartesian 2D point (SRID 7203).
func Cartesian(x, y float64) map[string]any {
	return map[string]any{
		"kind": "point",
		"srid": SRIDCartesian2D,
		"crs":  "cartesian",
		"x":    x,
		"y":    y,
	}
}

// Cartesian3D builds a Cartesian 3D point (SRID 9157).
func Cartesian3D(x, y, z float64) map[string]any {
	return map[string]any{
		"kind": "point",
		"srid": SRIDCartesian3D,
		"crs":  "cartesian-3D",
		"x":    x,
		"y":    y,
		"z":    z,
	}
}

// WGS84 builds a WGS-84 geographic 2D point (SRID 4326). x is
// longitude, y is latitude; both naming conventions are exposed on
// the returned map so downstream readers can pick whichever is
// clearer.
func WGS84(longitude, latitude float64) map[string]any {
	return map[string]any{
		"kind":      "point",
		"srid":      SRIDWGS84_2D,
		"crs":       "WGS-84-2D",
		"x":         longitude,
		"y":         latitude,
		"longitude": longitude,
		"latitude":  latitude,
	}
}

// WGS84_3D builds a WGS-84 geographic 3D point (SRID 4979). x is
// longitude, y is latitude, z is height in metres.
func WGS84_3D(longitude, latitude, height float64) map[string]any {
	return map[string]any{
		"kind":      "point",
		"srid":      SRIDWGS84_3D,
		"crs":       "WGS-84-3D",
		"x":         longitude,
		"y":         latitude,
		"z":         height,
		"longitude": longitude,
		"latitude":  latitude,
		"height":    height,
	}
}

// ---------------------------------------------------------------------------
// Guards
//
// All checks follow the same pattern: assert the value is a
// non-nil map[string]any and that the "kind" field matches one of
// the engine's documented tags. Each predicate is O(1) and makes no
// allocation.
// ---------------------------------------------------------------------------

// IsNode reports whether v is a node value returned by the engine.
func IsNode(v any) bool { return kindOf(v) == "node" }

// IsRelationship reports whether v is a relationship value.
func IsRelationship(v any) bool { return kindOf(v) == "relationship" }

// IsPath reports whether v is a path value.
func IsPath(v any) bool { return kindOf(v) == "path" }

// IsPoint reports whether v is a spatial point.
func IsPoint(v any) bool { return kindOf(v) == "point" }

// IsTemporal reports whether v is one of the six temporal kinds
// (date, time, localtime, datetime, localdatetime, duration).
func IsTemporal(v any) bool {
	switch kindOf(v) {
	case "date", "time", "localtime", "datetime", "localdatetime", "duration":
		return true
	default:
		return false
	}
}

// kindOf extracts the "kind" discriminator from a tagged value map.
// Returns "" for anything that isn't a map[string]any with a string
// kind field — meaning every guard falls through to false for plain
// primitives, slices, or nil values.
func kindOf(v any) string {
	m, ok := v.(map[string]any)
	if !ok || m == nil {
		return ""
	}
	k, _ := m["kind"].(string)
	return k
}
