(function_item
  name: (identifier) @name) @symbol.function

(struct_item
  name: (type_identifier) @name) @symbol.struct

(enum_item
  name: (type_identifier) @name) @symbol.enum

(trait_item
  name: (type_identifier) @name) @symbol.trait

(mod_item
  name: (identifier) @name) @symbol.module

(const_item
  name: (identifier) @name) @symbol.const

(static_item
  name: (identifier) @name) @symbol.static

(type_item
  name: (type_identifier) @name) @symbol.type
