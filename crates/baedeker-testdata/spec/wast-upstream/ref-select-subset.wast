;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/select.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_func.wast

(module
  (func $tf)
  (elem declare func $tf)

  (func (param funcref funcref i32) (result funcref)
    (select (result funcref) (local.get 0) (local.get 1) (local.get 2)))

  (func (param externref externref i32) (result externref)
    (select (result externref) (local.get 0) (local.get 1) (local.get 2)))

  (func (param i32) (result funcref)
    (select (result funcref)
      (ref.func $tf)
      (ref.null func)
      (local.get 0)))

  (func (param i32) (result i32)
    (ref.is_null
      (select (result funcref)
        (ref.func $tf)
        (ref.null func)
        (local.get 0)))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param i32) (result (ref null $t1))
    (select (result (ref null $t1))
      (ref.func $f)
      (ref.null $t1)
      (local.get 0))))

(assert_invalid
  (module
    (func (param externref funcref i32) (result externref)
      (select (result externref) (local.get 0) (local.get 1) (local.get 2))))
  "type mismatch"
)

(assert_invalid
  (module
    (func (param externref externref externref) (result externref)
      (select (result externref) (local.get 0) (local.get 1) (local.get 2))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i64)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param i32) (result (ref null $t1))
      (select (result (ref null $t1))
        (ref.func $f)
        (ref.null $t1)
        (local.get 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param i32) (result (ref $t0))
      (select (result (ref null $t0))
        (ref.func $f)
        (ref.null $t0)
        (local.get 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i64)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param i32) (result (ref null $t1))
      (block (result (ref null $t1))
        (select (result (ref null $t0))
          (ref.func $f)
          (ref.null $t0)
          (local.get 0)))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i64)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param i32) (result (ref null $t1))
      (if (result (ref null $t1))
        (local.get 0)
        (then
          (select (result (ref null $t0))
            (ref.func $f)
            (ref.null $t0)
            (local.get 0)))
        (else
          (ref.null $t1)))))
  "type mismatch"
)
