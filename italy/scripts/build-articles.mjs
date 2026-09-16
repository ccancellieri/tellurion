import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(scriptDir, "..");
const sourceDir = path.join(root, "content", "articles");
const releaseBase =
  "https://github.com/ccancellieri/tellurion-italy-demo/releases/download/demo-v0.2.0";
const siteBase = "https://ccancellieri.github.io/tellurion-italy-demo";
const bundleUrl = `${releaseBase}/tellurion-italy-demo-v0.2.0.zip`;

const articles = [
  ["01-on-prem-is-a-choice.md", "on-prem-is-a-choice", "01-tellurion-italy.png"],
  ["02-two-open-layers-in-rome.md", "two-open-layers-in-rome", "02-tellurion-italy.png"],
  ["03-remote-cog-local-computation.md", "remote-cog-local-computation", "03-tellurion-italy.png"],
  ["04-the-analysis-boundary.md", "the-analysis-boundary", "04-tellurion-italy.png"],
  ["05-what-the-number-means.md", "what-the-number-means", "05-tellurion-italy.png"],
  ["06-reproduce-before-you-believe.md", "reproduce-before-you-believe", "06-tellurion-italy.png"],
  ["07-tools-have-different-jobs.md", "tools-have-different-jobs", "07-tellurion-italy.png"],
  ["08-benchmarking-without-inventing-results.md", "benchmarking-without-inventing-results", "08-tellurion-italy.png"],
  ["09-standards-with-local-control.md", "standards-with-local-control", "09-tellurion-italy.png"],
  ["10-a-better-next-test.md", "a-better-next-test", "10-tellurion-italy.png"],
];

const linkTargets = new Map([
  ["../README.md", "https://github.com/ccancellieri/tellurion-italy-demo#readme"],
  ["../README.md#reproduce-in-two-commands", "../../#reproduce"],
  ["../README.md#download-and-run-tellurion-on-premise", "../../#reproduce"],
  ["../RESULTS.md", "https://github.com/ccancellieri/tellurion-italy-demo/blob/main/RESULTS.md"],
  ["../analyze.py", "https://github.com/ccancellieri/tellurion-italy-demo/blob/main/reproduce/analyze.py"],
  ["../benchmark/REPORT.md", "https://github.com/ccancellieri/tellurion-italy-demo/blob/main/evidence/BENCHMARK-REPORT.md"],
  ["../benchmark/BENCHMARK-DESIGN.md", "https://github.com/ccancellieri/tellurion-italy-demo/blob/main/evidence/BENCHMARK-DESIGN.md"],
  ["../output/metrics.json", `${releaseBase}/metrics.json`],
  ["../output/italy-osm-copernicus-analysis.png", `${releaseBase}/italy-osm-copernicus-analysis.png`],
  ["../output/rome-roads.geojson", bundleUrl],
  ["../data/rome-osm-overpass.json", bundleUrl],
  ["../data/rome-worldcover.tif", bundleUrl],
]);

const escapeHtml = (value) =>
  value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");

function inline(markdown) {
  let html = escapeHtml(markdown);
  html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_, label, href) => {
    const target = linkTargets.get(href) ?? href;
    return `<a href="${escapeHtml(target)}">${label}</a>`;
  });
  html = html.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  html = html.replace(/`([^`]+)`/g, "<code>$1</code>");
  return html;
}

function markdownToHtml(markdown) {
  const cleaned = markdown
    .replace(/^# .+\n/, "")
    .replace(/\n\*\*Scheduled:[^\n]+\*\*\n/, "\n")
    .trim();
  const blocks = cleaned.split(/\n{2,}/);

  return blocks
    .map((block) => {
      const lines = block.split("\n");
      if (lines.every((line) => line.startsWith("- "))) {
        return `<ul>${lines.map((line) => `<li>${inline(line.slice(2))}</li>`).join("")}</ul>`;
      }
      if (block.startsWith("#")) {
        return `<p class="eyebrow">${inline(block)}</p>`;
      }
      return `<p>${inline(lines.join(" "))}</p>`;
    })
    .join("\n");
}

function plainDescription(markdown) {
  return markdown
    .replace(/^# .+\n/, "")
    .replace(/\n\*\*Scheduled:[^\n]+\*\*\n/, "\n")
    .split(/\n{2,}/)
    .find((block) => block && !block.startsWith("#"))
    .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
    .replaceAll("**", "")
    .slice(0, 220);
}

function articleTemplate({ title, description, body, slug, image, index }) {
  const previous = index > 0 ? articles[index - 1] : null;
  const next = index < articles.length - 1 ? articles[index + 1] : null;
  const pageUrl = `${siteBase}/articles/${slug}/`;
  const imageUrl = `${releaseBase}/${image}`;
  const previousLink = previous
    ? `<a href="../${previous[1]}/"><span class="article-number">Previous</span><br>${previous[1].replaceAll("-", " ")}</a>`
    : `<a href="../../"><span class="article-number">Project home</span><br>Tellurion Italy</a>`;
  const nextLink = next
    ? `<a href="../${next[1]}/"><span class="article-number">Next</span><br>${next[1].replaceAll("-", " ")}</a>`
    : `<a href="https://github.com/ccancellieri/tellurion-italy-demo/issues/new"><span class="article-number">Your turn</span><br>Propose the next test</a>`;

  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${escapeHtml(title)} — Tellurion Italy</title>
  <link rel="icon" href="../../favicon.svg" type="image/svg+xml">
  <meta name="description" content="${escapeHtml(description)}">
  <link rel="canonical" href="${pageUrl}">
  <meta property="og:type" content="article">
  <meta property="og:title" content="${escapeHtml(title)}">
  <meta property="og:description" content="${escapeHtml(description)}">
  <meta property="og:url" content="${pageUrl}">
  <meta property="og:image" content="${imageUrl}">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="627">
  <meta name="twitter:card" content="summary_large_image">
  <meta name="twitter:title" content="${escapeHtml(title)}">
  <meta name="twitter:description" content="${escapeHtml(description)}">
  <meta name="twitter:image" content="${imageUrl}">
  <link rel="stylesheet" href="../../styles.css">
</head>
<body>
  <a class="skip-link" href="#article">Skip to article</a>
  <header class="site-header">
    <nav class="nav" aria-label="Primary navigation">
      <a class="brand" href="../../"><span class="brand-mark" aria-hidden="true"></span>Tellurion Italy</a>
      <div class="nav-links">
        <a href="../../#evidence">Evidence</a>
        <a href="../../#comparison">Comparison</a>
        <a class="button" href="${bundleUrl}">Download kit</a>
      </div>
    </nav>
  </header>
  <main class="article-main" id="article">
    <p class="eyebrow">Field note ${String(index + 1).padStart(2, "0")} of 10</p>
    <h1>${escapeHtml(title)}</h1>
    <figure class="article-lead-image">
      <img src="${imageUrl}" alt="Article card for ${escapeHtml(title)}">
      <figcaption class="map-caption">Map data © OpenStreetMap contributors, ODbL 1.0. Land cover © ESA WorldCover project 2021 / Contains modified Copernicus Sentinel data (2021) processed by ESA WorldCover consortium, CC BY 4.0.</figcaption>
    </figure>
    <article class="article-body">${body}</article>
    <div class="caveat"><strong>Reproduce and challenge it:</strong> <a href="${bundleUrl}">download the versioned kit</a>, retain the source attribution, and <a href="https://github.com/ccancellieri/tellurion-italy-demo/issues/new">report your environment and metrics</a>.</div>
    <div class="source-list" style="margin-top: 1.5rem">
      <a href="../../#sources">Sources &amp; licences</a>
      <a href="https://www.openstreetmap.org/copyright">OSM / ODbL</a>
      <a href="https://esa-worldcover.org/en/data-access">ESA WorldCover / CC BY 4.0</a>
      <a href="https://doi.org/10.5281/zenodo.7254221">WorldCover 2021 DOI</a>
    </div>
    <nav class="article-nav" aria-label="Article sequence">${previousLink}${nextLink}</nav>
  </main>
  <footer class="site-footer">
    <div class="container footer-grid"><span>Tellurion Italy · field note ${index + 1}</span><span><a href="../../#sources">Sources &amp; licences</a> · <a href="https://github.com/ccancellieri/tellurion-italy-demo">Source</a></span></div>
  </footer>
</body>
</html>`;
}

const sitemapUrls = [`${siteBase}/`];
for (const [filename, slug, image] of articles) {
  const markdown = await readFile(path.join(sourceDir, filename), "utf8");
  const title = markdown.match(/^# (.+)$/m)?.[1] ?? slug.replaceAll("-", " ");
  const description = plainDescription(markdown);
  const outputDir = path.join(root, "articles", slug);
  await mkdir(outputDir, { recursive: true });
  await writeFile(
    path.join(outputDir, "index.html"),
    articleTemplate({
      title,
      description,
      body: markdownToHtml(markdown),
      slug,
      image,
      index: articles.findIndex((entry) => entry[1] === slug),
    }),
  );
  sitemapUrls.push(`${siteBase}/articles/${slug}/`);
}

const sitemap = `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
${sitemapUrls.map((url) => `  <url><loc>${url}</loc></url>`).join("\n")}
</urlset>
`;
await writeFile(path.join(root, "sitemap.xml"), sitemap);
