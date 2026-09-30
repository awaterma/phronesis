local Store = {}

function Store.new(path)
  return setmetatable({ path = path }, { __index = Store })
end

function Store:load()
  return read_file(self.path)
end

function Store:save(value)
  write_file(self.path, value)
end

function Store.reset() return {} end

return Store
