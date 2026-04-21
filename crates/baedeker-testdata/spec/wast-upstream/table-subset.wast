;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/table.wast

(module
  (table 1 funcref))

(module
  (table 1 externref))

(module
  (func $f)
  (func $g)
  (table $t funcref (elem (ref.func $f) (ref.null func) (ref.func $g))))

(module
  (import "M" "r" (global externref))
  (table $t externref (elem (global.get 0) (ref.null extern))))

(assert_invalid
  (module (elem (i32.const 0)))
  "unknown table"
)

(assert_invalid
  (module (elem (i32.const 0) $f) (func $f))
  "unknown table"
)

(assert_invalid
  (module
    (import "M" "r" (global externref))
    (table funcref (elem (global.get 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (import "M" "r" (global (mut externref)))
    (table externref (elem (global.get 0))))
  "constant expression required"
)

(assert_invalid
  (module
    (table externref (elem (global.get 0))))
  "unknown global 0"
)

(assert_malformed
  (module quote
    "(table $foo 1 funcref)"
    "(table $foo 1 funcref)"
  )
  "duplicate table"
)
