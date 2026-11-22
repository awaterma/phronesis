# Renders canonical markdown that has no YAML front matter through the
# shared layouts, so one docs/ source serves both GitHub and the website
# without the content migration having to add front matter to every file.
#
# Rules:
#   - A markdown file under docs/<section>/ (section ids come from
#     docs/_data/navigation.yml) becomes a site page: section from its
#     directory, layout `doc`, title from its first H1 (or filename).
#     The raw .md copy stops shipping; the rendered page takes its place.
#   - docs/<section>/README.md is the section index: permalink /<section>/
#     so it lands at the section root.
#   - docs/README.md is the documentation landing: permalink /docs/.
#   - Files that already have front matter are left to Jekyll's normal
#     pipeline (per-directory defaults in _config.yml apply).
#   - Excluded directories (archive, superpowers, ...) never reach the
#     generator because Jekyll drops them at read time.

class CanonicalPages < Jekyll::Generator
  def generate(site)
    nav = site.data.dig("navigation", "sections") || []
    section_ids = nav.map { |s| s["id"] }.compact

    keep, converted = [], []
    site.static_files.each do |f|
      rel = f.path.sub("#{site.source}/", "")
      if rel.end_with?(".md") && (canonical_page?(rel, section_ids) || rel == "README.md")
        converted << [f, rel]
      else
        keep << f
      end
    end

    converted.each do |_f, rel|
      parts = rel.split(File::SEPARATOR)
      page = Jekyll::Page.new(site, site.source, File.dirname(rel), File.basename(rel))
      raw = File.read(File.join(site.source, rel))
      page.content = raw
      first_heading = raw[/^#\s+(.+?)\s*$/, 1]
      page.data["layout"] ||= "doc"
      page.data["section"] ||= parts[0] if parts.size == 2 && section_ids.include?(parts[0])
      page.data["title"] ||= first_heading || File.basename(rel, ".md").split("-").map(&:capitalize).join(" ")
      if parts == ["README.md"]
        page.data["permalink"] = "/docs/"
      elsif parts.size == 2 && parts[1] == "README.md" && section_ids.include?(parts[0])
        page.data["permalink"] = "/#{parts[0]}/"
      end
      site.pages << page
    end

    site.static_files = keep

    # Pages that opt in via front matter may omit `title` (decision
    # records and specs deliberately do: the ADR schema has no website
    # keys). Derive the title from the first H1 heading exactly like the
    # shared checker (scripts/check_docs.py) derives it on the source
    # side, so sidebar and breadcrumb labels are never blank.
    site.pages.each do |page|
      next unless page.data["title"].nil?
      heading = page.content.to_s.lines.find { |line| line =~ /^#\s+/ }.to_s.sub(/^#\s+/, "").strip
      heading = nil if heading.empty?
      page.data["title"] = heading ||
        File.basename(page.path, ".*").split("-").map(&:capitalize).join(" ")
    end

    # Specs and plans quote Rust format strings containing "{{", which
    # Liquid would parse as template syntax. Jekyll honors a per-document
    # `render_with_liquid: false`; markdown still renders.
    (site.pages + site.documents).each do |document|
      next if document.data["render_with_liquid"] == true
      document.data["render_with_liquid"] = false if document.content.to_s.include?("{{")
    end

    Jekyll.logger.info "CanonicalPages:", "rendered #{converted.size} markdown file(s) without front matter"
  end

  def canonical_page?(rel, section_ids)
    parts = rel.split(File::SEPARATOR)
    parts.size == 2 && section_ids.include?(parts[0])
  end
end
