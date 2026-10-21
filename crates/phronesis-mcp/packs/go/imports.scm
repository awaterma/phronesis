; Go starter pack — import-spec captures. `@import_name` covers an explicit
; alias, a dot import (`.`), or a blank import (`_`); its absence is a
; separate pattern so a plain `import "fmt"` still matches.

(import_spec
  name: [(package_identifier) (dot) (blank_identifier)] @import_name
  path: [(interpreted_string_literal) (raw_string_literal)] @import_path) @import_site

(import_spec
  path: [(interpreted_string_literal) (raw_string_literal)] @import_path
  !name) @import_site
