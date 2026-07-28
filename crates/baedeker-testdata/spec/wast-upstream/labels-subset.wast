;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/labels.wast

(module
  (func (block $l (br $l)))
)

(module
  (func (export "block") (result i32)
    (block $exit (result i32)
      (br $exit (i32.const 1))
      (i32.const 0)))
)

(module
  (func (export "loop1") (result i32)
    (local $i i32)
    (local.set $i (i32.const 0))
    (block $exit (result i32)
      (loop $cont (result i32)
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (if (i32.eq (local.get $i) (i32.const 5))
          (then (br $exit (local.get $i))))
        (br $cont))))
)

(module
  (func (export "loop2") (result i32)
    (local $i i32)
    (local.set $i (i32.const 0))
    (block $exit (result i32)
      (loop $cont (result i32)
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (if (i32.eq (local.get $i) (i32.const 5))
          (then (br $cont)))
        (if (i32.eq (local.get $i) (i32.const 8))
          (then (br $exit (local.get $i))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $cont))))
)

(module
  (func (export "loop3") (result i32)
    (local $i i32)
    (local.set $i (i32.const 0))
    (block $exit (result i32)
      (loop $cont (result i32)
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (if (i32.eq (local.get $i) (i32.const 5))
          (then (br $exit (local.get $i))))
        (local.get $i))))
)

(module
  (func (export "loop4") (param $max i32) (result i32)
    (local $i i32)
    (local.set $i (i32.const 1))
    (block $exit (result i32)
      (loop $cont (result i32)
        (local.set $i (i32.add (local.get $i) (local.get $i)))
        (if (i32.gt_u (local.get $i) (local.get $max))
          (then (br $exit (local.get $i))))
        (br $cont))))
)

(module
  (func (export "if") (result i32)
    (local $i i32)
    (local.set $i (i32.const 0))
    (block
      (if $l
        (i32.const 1)
        (then (br $l) (local.set $i (i32.const 666))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (if $l
        (i32.const 1)
        (then (br $l) (local.set $i (i32.const 666)))
        (else (local.set $i (i32.const 888))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (if $l
        (i32.const 1)
        (then (br $l) (local.set $i (i32.const 666)))
        (else (local.set $i (i32.const 888))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (if $l
        (i32.const 0)
        (then (local.set $i (i32.const 888)))
        (else (br $l) (local.set $i (i32.const 666))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (if $l
        (i32.const 0)
        (then (local.set $i (i32.const 888)))
        (else (br $l) (local.set $i (i32.const 666))))
      (local.set $i (i32.add (local.get $i) (i32.const 1))))
    (local.get $i))
)

(module
  (func (export "if2") (result i32)
    (local $i i32)
    (local.set $i (i32.const 0))
    (block
      (if
        (i32.const 1)
        (then (br 0) (local.set $i (i32.const 666))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (if
        (i32.const 1)
        (then (br 0) (local.set $i (i32.const 666)))
        (else (local.set $i (i32.const 888))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (if
        (i32.const 1)
        (then (br 0) (local.set $i (i32.const 666)))
        (else (local.set $i (i32.const 888))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (if
        (i32.const 0)
        (then (local.set $i (i32.const 888)))
        (else (br 0) (local.set $i (i32.const 666))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (if
        (i32.const 0)
        (then (local.set $i (i32.const 888)))
        (else (br 0) (local.set $i (i32.const 666))))
      (local.set $i (i32.add (local.get $i) (i32.const 1))))
    (local.get $i))
)

(module
  (func (export "return") (param i32) (result i32)
    (block $default
      (block $1
        (block $0
          (br_table $0 $1 (local.get 0))
          (br $default))
        (return (i32.const 0))))
    (i32.const 2))
)

(module
  (func (export "br_if0") (result i32)
    (local $i i32)
    (local.set $i (i32.const 0))
    (block $outer (result i32)
      (block $inner
        (br_if $inner (i32.const 0))
        (local.set $i (i32.or (local.get $i) (i32.const 0x1)))
        (br_if $inner (i32.const 1))
        (local.set $i (i32.or (local.get $i) (i32.const 0x2))))
      (drop (br_if $outer
        (block (result i32)
          (local.set $i (i32.or (local.get $i) (i32.const 0x4)))
          (local.get $i))
        (i32.const 0)))
      (local.set $i (i32.or (local.get $i) (i32.const 0x8)))
      (drop (br_if $outer
        (block (result i32)
          (local.set $i (i32.or (local.get $i) (i32.const 0x10)))
          (local.get $i))
        (i32.const 1)))
      (local.set $i (i32.or (local.get $i) (i32.const 0x20)))
      (local.get $i)))
)

(module
  (func (export "br") (result i32)
    (block $l0 (result i32)
      (if (i32.const 1)
        (then (br $l0 (block $l1 (result i32) (br $l1 (i32.const 1)))))
        (else (block (drop (block $l1 (result i32) (br $l1 (i32.const 1)))))))
      (i32.const 1)))
)

(module
  (func (result i32)
    (i32.add
      (loop $l (result i32) (i32.const 1))
      (i32.const 1)))
)

(module
  (func (result i32)
    (loop (result i32)
      (br_if 0 (i32.const 0))
      (i32.const 3)))
)

(module
  (func (result i32)
    (block $l0 (result i32)
      (drop
        (br_if $l0
          (block $l1 (result i32)
            (br $l1 (i32.const 1)))
          (i32.const 1)))
      (i32.const 0)))
)

(module
  (func (result i32)
    (block $l0 (result i32)
      (if (i32.const 1)
        (then
          (drop
            (br_if $l0
              (block $l1 (result i32)
                (br $l1 (i32.const 1)))
              (i32.const 1)))))
      (i32.const 0)))
)

(module
  (func (export "br_if3") (result i32)
    (local $i1 i32)
    (drop
      (i32.add
        (block $l0 (result i32)
          (drop (br_if $l0
            (block (result i32) (local.set $i1 (i32.const 1)) (local.get $i1))
            (block (result i32) (local.set $i1 (i32.const 2)) (local.get $i1))))
          (i32.const 0))
        (i32.const 0)))
    (local.get $i1))
)

(module
  (func (result i32)
    (block $l1 (result i32)
      (i32.xor (br $l1 (i32.const 1)) (i32.const 2))))
)

(module
  (func (export "redefinition") (result i32)
    (block $l1 (result i32)
      (i32.add
        (block $l1 (result i32) (i32.const 2))
        (block $l1 (result i32) (br $l1 (i32.const 3))))))
)

(module
  (func (param i32) (result i32)
    (block $ret (result i32)
      (i32.mul (i32.const 10)
        (block $exit (result i32)
          (block $0
            (block $default
              (block $3
                (block $2
                  (block $1
                    (br_table $0 $1 $2 $3 $default (local.get 0))))
                (br $exit (i32.const 2)))
              (br $ret (i32.const 3))))
          (i32.const 5))))))
