;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/labels.wast

(module
  (func (block $l (br $l)))
)
