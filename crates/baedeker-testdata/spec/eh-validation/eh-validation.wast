;; Exception-handling validation fixtures (EH unit 2).

(module
  (type (func))
  (type (func (param i32)))
  (tag (type 1))
  (tag $t2 (type 0))
  (export "t2" (tag $t2))
)

(module
  (type (func))
  (import "" "" (tag (type 0)))
)

;; tag type must be a func type with empty results
(assert_invalid
  (module (type (func (result i32))) (tag (type 0)))
  "non-empty tag result type"
)
(assert_invalid
  (module (type (func (result i32))) (import "" "" (tag (type 0))))
  "non-empty tag result type"
)

;; unknown tag referenced by throw
(assert_invalid
  (module (tag) (func (throw 1)))
  "unknown tag"
)

;; catch label arity mismatch: label wants i32, tag has no params
(assert_invalid
  (module
    (tag)
    (func (result i32)
      (block $h (result i32)
        (try_table (catch 0 $h) (nop))
        (i32.const 1)
      )
    )
  )
  "type mismatch"
)

;; catch label arity mismatch: label wants nothing, tag has i32 param
(assert_invalid
  (module
    (tag (param i32))
    (func
      (block $h
        (try_table (catch 0 $h) (i32.const 1) (throw 0))
      )
    )
  )
  "type mismatch"
)

;; catch_ref label arity mismatch: missing the exnref slot
(assert_invalid
  (module
    (tag (param i32))
    (func (result i32)
      (block $h (result i32)
        (try_table (catch_ref 0 $h) (i32.const 1) (throw 0))
        (i32.const 7)
      )
    )
  )
  "type mismatch"
)

;; catch_all label must have empty types
(assert_invalid
  (module
    (func (result i32)
      (block $h (result i32)
        (try_table (catch_all $h) (nop))
        (i32.const 1)
      )
    )
  )
  "type mismatch"
)

;; catch_all_ref label must be exactly [exnref]
(assert_invalid
  (module
    (func
      (block $h
        (try_table (catch_all_ref $h) (nop))
      )
    )
  )
  "type mismatch"
)

;; catch label type mismatch: label [f32] vs tag param [i32]
(assert_invalid
  (module
    (tag (param i32))
    (func (result f32)
      (block $h (result f32)
        (try_table (catch 0 $h) (nop))
        (f32.const 1)
      )
    )
  )
  "type mismatch"
)

;; unknown catch label
(assert_invalid
  (module (tag) (func (try_table (catch 0 3))))
  "unknown label"
)

;; throw parameter mismatches
(assert_invalid
  (module (tag (param i32)) (func (throw 0)))
  "type mismatch"
)
(assert_invalid
  (module (tag (param i32)) (func (f32.const 1) (throw 0)))
  "type mismatch"
)

;; throw_ref operand must be exnref
(assert_invalid
  (module (func (i32.const 1) (throw_ref)))
  "type mismatch"
)
(assert_invalid
  (module (func (local exnref) (local.get 0) (drop) (i32.const 2) (throw_ref)))
  "type mismatch"
)

;; export bounds
(assert_invalid
  (module (tag) (export "t" (tag 1)))
  "unknown tag"
)

;; valid: exnref locals, catch into matching label, throw_ref on nullable exnref
(module
  (tag (param i32))
  (func (param exnref) (result i32)
    (local exnref)
    (block $h (result i32)
      (try_table (catch 0 $h) (i32.const 1) (throw 0))
      (i32.const 2)
    )
  )
  (func (param exnref) (local.get 0) (throw_ref))
)
