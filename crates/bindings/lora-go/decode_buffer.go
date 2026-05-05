package lora

import (
	"encoding/binary"
	"fmt"
	"math"
)

// Wire-format tags. Must stay in lockstep with crates/bindings/lora-binding-buffer/src/lib.rs.
const (
	tagNull          uint8 = 0x00
	tagFalse         uint8 = 0x01
	tagTrue          uint8 = 0x02
	tagI32           uint8 = 0x03
	tagI64           uint8 = 0x04
	tagF64           uint8 = 0x05
	tagString        uint8 = 0x06
	tagList          uint8 = 0x07
	tagMap           uint8 = 0x08
	tagNode          uint8 = 0x09
	tagRelationship  uint8 = 0x0A
	tagPath          uint8 = 0x0B
	tagDate          uint8 = 0x0C
	tagTime          uint8 = 0x0D
	tagLocalTime     uint8 = 0x0E
	tagDatetime      uint8 = 0x0F
	tagLocalDatetime uint8 = 0x10
	tagDuration      uint8 = 0x11
	tagPoint         uint8 = 0x12
	tagVector        uint8 = 0x13
	tagBinary        uint8 = 0x14
)

const (
	vectorFloat64 uint8 = 0
	vectorFloat32 uint8 = 1
	vectorInt64   uint8 = 2
	vectorInt32   uint8 = 3
	vectorInt16   uint8 = 4
	vectorInt8    uint8 = 5
)

// reader walks a bounded byte slice. It panics on out-of-bounds reads;
// callers wrap the decode in recover to surface a clean error.
type reader struct {
	buf []byte
	off int
}

func (r *reader) u8() uint8 {
	v := r.buf[r.off]
	r.off++
	return v
}

func (r *reader) u32() uint32 {
	v := binary.LittleEndian.Uint32(r.buf[r.off:])
	r.off += 4
	return v
}

func (r *reader) i32() int32 {
	v := int32(binary.LittleEndian.Uint32(r.buf[r.off:]))
	r.off += 4
	return v
}

func (r *reader) i64() int64 {
	v := int64(binary.LittleEndian.Uint64(r.buf[r.off:]))
	r.off += 8
	return v
}

func (r *reader) f64() float64 {
	v := math.Float64frombits(binary.LittleEndian.Uint64(r.buf[r.off:]))
	r.off += 8
	return v
}

func (r *reader) f32() float32 {
	v := math.Float32frombits(binary.LittleEndian.Uint32(r.buf[r.off:]))
	r.off += 4
	return v
}

func (r *reader) i16() int16 {
	v := int16(binary.LittleEndian.Uint16(r.buf[r.off:]))
	r.off += 2
	return v
}

func (r *reader) str() string {
	n := int(r.u32())
	// Aliasing the underlying buffer would be faster but the buffer is
	// freed via lora_bytes_free as soon as decode returns; copy.
	s := string(r.buf[r.off : r.off+n])
	r.off += n
	return s
}

func sridToCRS(srid uint32) string {
	switch srid {
	case 7203:
		return "cartesian"
	case 9157:
		return "cartesian-3D"
	case 4326:
		return "WGS-84-2D"
	case 4979:
		return "WGS-84-3D"
	default:
		return "cartesian"
	}
}

func (r *reader) value() any {
	tag := r.u8()
	switch tag {
	case tagNull:
		return nil
	case tagFalse:
		return false
	case tagTrue:
		return true
	case tagI32:
		return int64(r.i32())
	case tagI64:
		return r.i64()
	case tagF64:
		return r.f64()
	case tagString:
		return r.str()
	case tagList:
		n := int(r.u32())
		out := make([]any, n)
		for i := 0; i < n; i++ {
			out[i] = r.value()
		}
		return out
	case tagMap:
		n := int(r.u32())
		out := make(map[string]any, n)
		for i := 0; i < n; i++ {
			k := r.str()
			out[k] = r.value()
		}
		return out
	case tagNode:
		return map[string]any{
			"kind":       "node",
			"id":         r.i64(),
			"labels":     []any{},
			"properties": map[string]any{},
		}
	case tagRelationship:
		return map[string]any{
			"kind": "relationship",
			"id":   r.i64(),
		}
	case tagPath:
		nN := int(r.u32())
		nodes := make([]any, nN)
		for i := 0; i < nN; i++ {
			nodes[i] = r.i64()
		}
		nR := int(r.u32())
		rels := make([]any, nR)
		for i := 0; i < nR; i++ {
			rels[i] = r.i64()
		}
		return map[string]any{"kind": "path", "nodes": nodes, "rels": rels}
	case tagDate:
		return map[string]any{"kind": "date", "iso": r.str()}
	case tagTime:
		return map[string]any{"kind": "time", "iso": r.str()}
	case tagLocalTime:
		return map[string]any{"kind": "localtime", "iso": r.str()}
	case tagDatetime:
		return map[string]any{"kind": "datetime", "iso": r.str()}
	case tagLocalDatetime:
		return map[string]any{"kind": "localdatetime", "iso": r.str()}
	case tagDuration:
		return map[string]any{"kind": "duration", "iso": r.str()}
	case tagPoint:
		hasZ := r.u8() != 0
		srid := r.u32()
		x := r.f64()
		y := r.f64()
		out := map[string]any{
			"kind": "point",
			"srid": int64(srid),
			"crs":  sridToCRS(srid),
			"x":    x,
			"y":    y,
		}
		if hasZ {
			out["z"] = r.f64()
		}
		if srid == 4326 || srid == 4979 {
			out["longitude"] = x
			out["latitude"] = y
			if hasZ {
				out["height"] = out["z"]
			}
		}
		return out
	case tagVector:
		coordTag := r.u8()
		dim := int(r.u32())
		values := make([]any, dim)
		var coordType string
		switch coordTag {
		case vectorFloat64:
			coordType = "FLOAT64"
			for i := 0; i < dim; i++ {
				values[i] = r.f64()
			}
		case vectorFloat32:
			coordType = "FLOAT32"
			for i := 0; i < dim; i++ {
				values[i] = float64(r.f32())
			}
		case vectorInt64:
			coordType = "INTEGER"
			for i := 0; i < dim; i++ {
				values[i] = r.i64()
			}
		case vectorInt32:
			coordType = "INTEGER32"
			for i := 0; i < dim; i++ {
				values[i] = int64(r.i32())
			}
		case vectorInt16:
			coordType = "INTEGER16"
			for i := 0; i < dim; i++ {
				values[i] = int64(r.i16())
			}
		case vectorInt8:
			coordType = "INTEGER8"
			for i := 0; i < dim; i++ {
				values[i] = int64(int8(r.u8()))
			}
		default:
			panic(fmt.Sprintf("lora: unknown vector coord type 0x%02x", coordTag))
		}
		return map[string]any{
			"kind":           "vector",
			"dimension":      int64(dim),
			"coordinateType": coordType,
			"values":         values,
		}
	case tagBinary:
		segCount := int(r.u32())
		segments := make([]any, segCount)
		total := 0
		for i := 0; i < segCount; i++ {
			n := int(r.u32())
			seg := make([]any, n)
			for j := 0; j < n; j++ {
				seg[j] = int64(r.buf[r.off+j])
			}
			r.off += n
			segments[i] = seg
			total += n
		}
		return map[string]any{
			"kind":     "binary",
			"length":   int64(total),
			"segments": segments,
		}
	default:
		panic(fmt.Sprintf("lora: unknown tag 0x%02x", tag))
	}
}

// decodeBuffer parses the lora-binding-buffer wire format into the
// canonical {Columns, Rows} Result.
func decodeBuffer(buf []byte) (result *Result, err error) {
	defer func() {
		if rec := recover(); rec != nil {
			result = nil
			err = fmt.Errorf("lora: decode buffer: %v", rec)
		}
	}()

	if len(buf) < 12 {
		return nil, fmt.Errorf("lora: result buffer too short (%d bytes)", len(buf))
	}
	if buf[0] != 'L' || buf[1] != 'R' || buf[2] != '1' || buf[3] != 0x00 {
		return nil, fmt.Errorf("lora: invalid result buffer magic")
	}

	r := &reader{buf: buf, off: 4}
	colCount := int(r.u32())
	columns := make([]string, colCount)
	for i := 0; i < colCount; i++ {
		columns[i] = r.str()
	}
	rowCount := int(r.u32())
	rows := make([]Row, rowCount)
	for i := 0; i < rowCount; i++ {
		row := make(Row, colCount)
		for j := 0; j < colCount; j++ {
			row[columns[j]] = r.value()
		}
		rows[i] = row
	}

	return &Result{Columns: columns, Rows: rows}, nil
}
