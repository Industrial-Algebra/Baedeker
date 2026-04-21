;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/elem.wast

(module
  (table 1 externref)
  (elem (i32.const 0) externref (ref.null extern)))

(module
  (table 2 funcref)
  (func $f)
  (elem (table 0) (i32.const 0) funcref (ref.func $f) (ref.null func)))

(module
  (func $f)
  (elem declare funcref (ref.func $f) (ref.null func)))

(module
  (global (import "spectest" "global_i32") i32)
  (table 1 funcref)
  (func $f)
  (elem (global.get 0) $f))

(module
  (import "test" "f" (global funcref))
  (table 10 funcref)
  (elem (offset (i32.const 0)) funcref (global.get 0)))

(module
  (import "test" "r" (global externref))
  (table 2 externref)
  (elem (i32.const 0) externref (global.get 0) (ref.null extern)))

(assert_invalid
  (module
    (func)
    (table 1 externref)
    (elem (i32.const 0) funcref (ref.func 0)))
  "type mismatch"
)

(assert_invalid
  (module
    (func $f)
    (elem (i32.const 0) $f))
  "unknown table"
)

(assert_invalid
  (module
    (table 1 funcref)
    (elem (i64.const 0)))
  "type mismatch"
)

(assert_invalid
  (module
    (table 1 funcref)
    (elem (i32.const 0) funcref (ref.null extern)))
  "type mismatch"
)

(assert_invalid
  (module
    (import "test" "r" (global externref))
    (table 1 funcref)
    (elem (i32.const 0) funcref (global.get 0)))
  "type mismatch"
)

(assert_invalid
  (module
    (global $g (import "test" "g") (mut i32))
    (table 1 funcref)
    (elem (global.get $g)))
  "constant expression required"
)

(assert_invalid
  (module
    (import "test" "r" (global (mut externref)))
    (table 1 externref)
    (elem (i32.const 0) externref (global.get 0)))
  "constant expression required"
)

(assert_invalid
   (module
     (table 1 funcref)
     (elem (global.get 0)))
   "unknown global 0"
)

(assert_invalid
  (module
    (global (import "test" "global-i32") i32)
    (table 1 funcref)
    (elem (offset (global.get 0) (i32.const 0))))
  "type mismatch"
)
