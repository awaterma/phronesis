; Go starter pack — declaration-shaped captures. Query-only (empty `emit` in
; pack.json); `adapter.rhai` interprets every match. Comments, string/raw
; string contents, and identifiers inside them are not part of the syntax
; tree, so they never produce a capture here.

; Package clause: names the file's package scope.
(package_clause
  (package_identifier) @pkg_name) @pkg_decl

; Free function declarations (including variadic and generic parameter
; lists; `type_parameters` itself is not captured — generics affect only
; receiver-type normalization in this starter pack).
(function_declaration
  name: (identifier) @func_name
  type_parameters: (type_parameter_list)? @func_type_params
  parameters: (parameter_list
    (parameter_declaration
      name: (identifier)? @func_param_name
      type: (_) @func_param_type)*
    (variadic_parameter_declaration
      name: (identifier)? @func_param_name
      type: (_) @func_param_type)*)
  result: (_)? @func_result
  body: (block)? @func_body) @func_decl

; Method declarations: the receiver's declared type is captured whole so the
; adapter can normalize pointer/value spelling and strip generic type
; arguments (`*List[T]` -> `List`).
(method_declaration
  receiver: (parameter_list
    (parameter_declaration type: (_) @method_receiver_type))
  name: (field_identifier) @method_name
  parameters: (parameter_list
    (parameter_declaration
      name: (identifier) @method_param_name
      type: (_) @method_param_type)*
    (variadic_parameter_declaration
      name: (identifier)? @method_param_name
      type: (_) @method_param_type)*)
  body: (block)? @method_body) @method_decl

; Local bindings that may shadow a package-level function name. Matched
; anywhere in the file (not anchored to a specific function); the adapter
; attributes each one to its enclosing declaration by span containment,
; which is the only way to handle arbitrarily nested blocks with one
; non-recursive query.
(short_var_declaration
  left: (expression_list (identifier) @local_name)) @local_site

(var_spec
  name: (identifier) @local_name) @local_site
