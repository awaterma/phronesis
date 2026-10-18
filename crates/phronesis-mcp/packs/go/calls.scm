; Go starter pack — call-shaped captures. Unqualified calls are candidates
; for same-package resolution (subject to the adapter's shadow checks);
; selector calls (`recv.Method()`, `pkg.Func()`) remain raw references in
; this starter pack per the design's v1 scope.

(call_expression
  function: (identifier) @call_name) @call_site

(call_expression
  function: (parenthesized_expression (identifier) @call_name)) @call_site

(call_expression
  function: (selector_expression
    operand: (identifier) @call_receiver
    field: (field_identifier) @call_field)) @call_site
