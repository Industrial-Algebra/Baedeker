;; memory64 / table64 validation fixtures (memory64 unit 2).
;;
;; Operand types follow the reference interpreter (valid.ml @ dfa3f32a):
;; `numtype_of_addrtype` gives the memory/table index type, and `min at1 at2`
;; picks the narrower of two address types (i32 unless both are i64).
;;
;; This unit changes validation only: valid modules are decoded and validated,
;; never instantiated (runtime/IR semantics land in unit 3).

;; ---------------------------------------------------------------------------
;; Valid: memory64 operand types
;; ---------------------------------------------------------------------------

;; i64 address on load/store, i64 results from size/grow.
(module
  (memory i64 1)
  (func (param i64) (result i64)
    (i64.store (local.get 0) (i64.const 0))
    (drop (i64.load (local.get 0)))
    (drop (memory.grow (i64.const 1)))
    (memory.size)
  )
)

;; Narrower load results are unchanged: result i32, address i64.
(module
  (memory i64 1)
  (func (param i64) (result i32)
    (i32.load8_u (local.get 0))
  )
)

;; SIMD load/store and lane accesses take the memory's address type.
(module
  (memory i64 1)
  (func (param i64) (result v128)
    (v128.store (local.get 0) (v128.const i32x4 0 0 0 0))
    (drop (v128.load8_lane 0 (local.get 0) (v128.const i32x4 0 0 0 0)))
    (v128.load (local.get 0))
  )
)

;; memory.init: dst = memory address type; src/count always i32.
;; memory.copy: all operands i64. memory.fill: value always i32.
(module
  (memory i64 1)
  (data $d "\00")
  (func (param i64) (result i64)
    (memory.init $d (local.get 0) (i32.const 0) (i32.const 1))
    (memory.copy (i64.const 0) (i64.const 0) (i64.const 0))
    (memory.fill (i64.const 0) (i32.const 0) (i64.const 0))
    (i64.const 0)
  )
)

;; ---------------------------------------------------------------------------
;; Valid: table64 operand types
;; ---------------------------------------------------------------------------

;; get/set/size/grow/fill use the table's address type; grow's fill value is
;; the element type.
(module
  (table i64 1 funcref)
  (func (param i64) (result i64)
    (drop (table.get (local.get 0)))
    (table.set (local.get 0) (ref.null func))
    (drop (table.size))
    (drop (table.grow (ref.null func) (i64.const 1)))
    (table.fill (i64.const 0) (ref.null func) (i64.const 0))
    (i64.const 0)
  )
)

;; table.copy i64 -> i64 has i64 operands (count = min(i64, i64)).
;; call_indirect on a 64-bit table takes an i64 index.
(module
  (type $t (func))
  (table $a i64 1 funcref)
  (table $b i64 1 funcref)
  (func (param i64) (result i64)
    (table.copy $a $b (i64.const 0) (i64.const 0) (i64.const 0))
    (call_indirect (type $t) (local.get 0))
    (i64.const 0)
  )
)

;; Mixed table.copy: count = min(i32, i64) = i32; dst/src follow each table.
(module
  (table $t32 1 funcref)
  (table $t64 i64 1 funcref)
  (func (result i32)
    (table.copy $t32 $t64 (i32.const 0) (i64.const 0) (i32.const 0))
    (table.copy $t64 $t32 (i64.const 0) (i32.const 0) (i32.const 0))
    (table.copy $t64 $t64 (i64.const 0) (i64.const 0) (i64.const 0))
    (i32.const 0)
  )
)

;; table.init: dst = table address type; src/count always i32.
(module
  (table i64 1 funcref)
  (elem func $f)
  (func $f)
  (func (param i64) (result i64)
    (table.init 0 0 (local.get 0) (i32.const 0) (i32.const 0))
    (i64.const 0)
  )
)

;; ---------------------------------------------------------------------------
;; Valid: active segment offsets are the target's address type
;; ---------------------------------------------------------------------------

;; i64 data offset on an i64 memory and i64 elem offset on an i64 table.
(module
  (memory i64 1)
  (data (i64.const 0) "x")
  (table i64 1 funcref)
  (elem (i64.const 0) func $f)
  (func $f)
)

;; ---------------------------------------------------------------------------
;; Valid: limit ranges
;; ---------------------------------------------------------------------------

;; i64 memory: 2^48 pages is the inclusive maximum.
(module (memory i64 0x1_0000_0000_0000))
(module (memory i64 0 0x1_0000_0000_0000))

;; i64 table: range is unbounded (u64), so 2^32 and u64::MAX are accepted.
(module (table i64 0x1_0000_0000 funcref))
(module (table i64 0xffff_ffff_ffff_ffff funcref))

;; i32 table: 2^32-1 pages is the inclusive maximum.
(module (table 0 0xffff_ffff funcref))

;; ---------------------------------------------------------------------------
;; Invalid: 64-bit memory operand types
;; ---------------------------------------------------------------------------

(assert_invalid
  (module (memory i64 1) (func (param i32) (result i32) (i32.load (local.get 0))))
  "type mismatch"
)
(assert_invalid
  (module (memory i64 1) (func (param i32) (result i64) (memory.grow (local.get 0))))
  "type mismatch"
)
(assert_invalid
  (module (memory i64 1) (func (result i32) (memory.size)))
  "type mismatch"
)
(assert_invalid
  (module (memory i64 1) (func (memory.fill (i64.const 0) (i64.const 0) (i64.const 0))))
  "type mismatch"
)
(assert_invalid
  (module (memory i64 1) (data "\00")
    (func (memory.init 0 (i64.const 0) (i64.const 0) (i32.const 0))))
  "type mismatch"
)
(assert_invalid
  (module (memory i64 1) (data "\00")
    (func (memory.init 0 (i64.const 0) (i32.const 0) (i64.const 0))))
  "type mismatch"
)
(assert_invalid
  (module (memory i64 1)
    (func (memory.copy (i64.const 0) (i64.const 0) (i32.const 0))))
  "type mismatch"
)

;; ---------------------------------------------------------------------------
;; Invalid: 64-bit table operand types
;; ---------------------------------------------------------------------------

(assert_invalid
  (module (table i64 1 funcref) (func (result funcref) (table.get (i32.const 0))))
  "type mismatch"
)
(assert_invalid
  (module (table i64 1 funcref) (func (table.set (i32.const 0) (ref.null func))))
  "type mismatch"
)
(assert_invalid
  (module (table i64 1 funcref) (func (result i32) (table.size)))
  "type mismatch"
)
(assert_invalid
  (module (table i64 1 funcref) (func (table.grow (ref.null func) (i32.const 1))))
  "type mismatch"
)
(assert_invalid
  (module (table i64 1 funcref)
    (func (table.fill (i32.const 0) (ref.null func) (i64.const 0))))
  "type mismatch"
)
(assert_invalid
  (module (table i64 1 funcref)
    (func (table.copy (i64.const 0) (i64.const 0) (i32.const 0))))
  "type mismatch"
)
(assert_invalid
  (module (table i64 1 funcref) (elem func $f) (func $f)
    (func (table.init 0 0 (i64.const 0) (i64.const 0) (i32.const 0))))
  "type mismatch"
)
(assert_invalid
  (module (type $t (func)) (table i64 1 funcref)
    (func (call_indirect (type $t) (i32.const 0))))
  "type mismatch"
)

;; Mixed table.copy count must be min(i32, i64) = i32; an i64 count is wrong.
(assert_invalid
  (module
    (table $t32 1 funcref)
    (table $t64 i64 1 funcref)
    (func (table.copy $t32 $t64 (i32.const 0) (i64.const 0) (i64.const 0))))
  "type mismatch"
)

;; ---------------------------------------------------------------------------
;; Invalid: segment offset type mismatches
;; ---------------------------------------------------------------------------

(assert_invalid
  (module (memory i64 1) (data (i32.const 0) "x"))
  "type mismatch"
)
(assert_invalid
  (module (table i64 1 funcref) (elem (i32.const 0) func $f) (func $f))
  "type mismatch"
)
(assert_invalid
  (module (memory 1) (data (i64.const 0) "x"))
  "type mismatch"
)

;; ---------------------------------------------------------------------------
;; Invalid: limit ranges
;; ---------------------------------------------------------------------------

;; i64 memory: one past 2^48 pages is rejected (min and max).
(assert_invalid
  (module (memory i64 0x1_0000_0000_0001))
  "memory size"
)
(assert_invalid
  (module (memory i64 0 0x1_0000_0000_0001))
  "memory size"
)
;; i32 memory: the pre-existing 2^16-page bound is unchanged.
(assert_invalid
  (module (memory 65537))
  "memory size"
)
;; i64 table: only min <= max applies.
(assert_invalid
  (module (table i64 1 0 funcref))
  "size minimum must not be greater than maximum"
)

;; ---------------------------------------------------------------------------
;; Malformed: an i32 table's limits are u32-encoded, so a value above 2^32-1
;; cannot be represented and is rejected during decode (not validation).
;; ---------------------------------------------------------------------------

(assert_malformed
  (module (table 0x1_0000_0000 funcref))
  "integer too large"
)
