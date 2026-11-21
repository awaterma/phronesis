# One content source: GitHub-style relative markdown links become site
# URLs during rendering. Pages read the same on GitHub (links point at
# sibling .md files) and on the website (links point at rendered pages).
#
# Conversion rules (pretty permalinks put pages a directory deeper than
# their source path, so relative non-page links cannot survive as-is):
#   - relative links resolving to an opted-in rendered page -> absolute
#     site URL
#   - directory links resolving to a rendered section index -> absolute
#     site URL
#   - links resolving to a static file that ships on the site -> absolute
#     site URL (the file exists in the artifact, but pretty permalinks
#     broke the relative depth)
#   - any other link inside the repository (archive copies, crate sources,
#     configs, raw documents) -> GitHub blob/tree URL, preserving the
#     fragment, because GitHub remains the canonical reading surface for
#     files the website does not publish
#   - links leaving the repository -> unchanged
# Anything left unconverted and unresolvable is flagged by the
# rendered-link check in scripts/check_docs.py.

GITHUB_BASE = "https://github.com/awaterma/phronesis"

Jekyll::Hooks.register [:pages, :documents], :post_render do |page|
  site = page.site
  baseurl = site.config["baseurl"].to_s.chomp("/")
  source_root = File.expand_path(site.source)
  repo_root = File.expand_path("..", source_root)
  # Prefix helpers that also work when a root is "/" (container mounts).
  source_prefix = source_root.end_with?("/") ? source_root : source_root + "/"
  repo_prefix = repo_root.end_with?("/") ? repo_root : repo_root + "/"

  page_index = {}
  (site.pages + site.documents).each do |p|
    src = p.respond_to?(:relative_path) ? p.relative_path : p.path
    page_index[src] = p.url
  end

  static_rels = site.static_files.map do |f|
    f.path.start_with?(source_prefix) ? f.path[source_prefix.length..] : nil
  end.compact

  # Jekyll gives pages a relative_path like "specs/SPEC-x.md"; expand it
  # against site.source, otherwise File.expand_path of a link that leaves
  # docs/ resolves against the process CWD and escapes the site root.
  source_dir = File.expand_path(
    File.dirname(page.respond_to?(:relative_path) ? page.relative_path : page.path),
    site.source)

  convert = lambda do |url|
    fragment = url[/#.*\z/]
    path = url.sub(/#.*\z/, "")
    return url if path.empty?
    return url if path =~ %r{\A[a-z]+://}i || path.start_with?("mailto:", "data:")
    # Site-absolute URLs (site.baseurl-prefixed links emitted by the
    # shared layouts themselves, CSS, section indexes) are already
    # correct; only links written relative to the markdown source are
    # candidates for conversion.
    return url if path.start_with?("/")

    expanded = File.expand_path(path, source_dir)
    if expanded.start_with?(source_prefix)
      target = expanded[source_prefix.length..]
      rendered = page_index[target]
      rendered ||= page_index["#{target}/README.md"] if File.directory?(expanded)
      unless rendered.nil?
        target_url = "#{baseurl}#{rendered}"
        return fragment ? "#{target_url}#{fragment}" : target_url
      end
      if static_rels.include?(target)
        target_url = "#{baseurl}/#{target}"
        return fragment ? "#{target_url}#{fragment}" : target_url
      end
      kind = File.directory?(expanded) ? "tree" : "blob"
      return "#{GITHUB_BASE}/#{kind}/main/docs/#{target}#{fragment}"
    end
    if expanded.start_with?(repo_prefix)
      repo_rel = expanded[repo_prefix.length..]
      kind = File.directory?(expanded) ? "tree" : "blob"
      return "#{GITHUB_BASE}/#{kind}/main/#{repo_rel}#{fragment}"
    end
    url
  end

  page.output = page.output.gsub(/(href|src)="([^"]*)"/) do |_match|
    attr, value = Regexp.last_match(1), Regexp.last_match(2)
    rewritten = convert.call(value)
    rewritten == value ? "#{attr}=\"#{value}\"" : "#{attr}=\"#{rewritten}\""
  end
end
