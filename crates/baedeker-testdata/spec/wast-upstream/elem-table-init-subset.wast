;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/elem.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/table.wast

(module
  (global $ofs (import "spectest" "global_i32") i32)
  (import "test" "r" (global externref))
  (table $t0 2 funcref)
  (table $t1 2 externref)
  (func $f)
  (elem (table $t0) (global.get $ofs) funcref (ref.func $f) (ref.null func))
  (elem (table $t1) (global.get $ofs) externref (global.get 1) (ref.null extern)))

(module
  (global $ofs (import "spectest" "global_i32") i32)
  (table $t0 2 funcref)
  (table $t1 2 funcref)
  (func $f)
  (elem (table $t1) (global.get $ofs) func $f))

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (elem (ref null $t0)
    (ref.func $f)))

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (global $g0 (ref $t0)
    (ref.func $f))
  (elem (ref null $t0)
    (global.get $g0)))

(assert_invalid
  (module
    (global $ofs (import "test" "g") (mut i32))
    (table 1 funcref)
    (table 1 externref)
    (func $f)
    (elem (table 1) (global.get $ofs) externref (ref.null extern)))
  "constant expression required"
)

(assert_invalid
  (module
    (import "test" "r" (global externref))
    (table 1 funcref)
    (table 1 externref)
    (func $f)
    (elem (table 1) (i32.const 0) funcref (global.get 0)))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (global $g (ref null $t0)
      (ref.null $t0))
    (elem (ref $t0)
      (global.get $g)))
  "type mismatch"
)

(assert_invalid
  (module
    (global $ofs (import "spectest" "global_i32") i32)
    (table 1 funcref)
    (func $f)
    (elem (table 1) (global.get $ofs) func $f))
  "unknown table"
)
