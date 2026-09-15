(() => {
  const dialog = document.getElementById("search-dialog");
  const trigger = document.getElementById("docs-search");
  const input = document.getElementById("search-input");
  const results = document.getElementById("search-results");
  const count = document.getElementById("search-count");
  if (!dialog || !trigger || !input || !results || !count || typeof dialog.showModal !== "function") return;
  document.documentElement.classList.add("search-ready");

  const sourceNodes = [...document.querySelectorAll("h2, h3, .matrix article, .manuals a")]
    .filter((node) => !dialog.contains(node));
  const entries = [];
  const seen = new Set();
  for (const node of sourceNodes) {
    const heading = node.matches("h2, h3") ? node : node.querySelector("h2, h3, strong");
    const section = node.closest("section[id]");
    const directLink = node.matches("a[href]") ? node : node.querySelector("a[href]");
    const href = directLink?.getAttribute("href") || (heading?.id ? `#${heading.id}` : section ? `#${section.id}` : "#overview");
    const title = heading?.textContent?.trim() || node.textContent?.trim() || "Documentation result";
    const text = node.textContent?.replace(/\s+/g, " ").trim() || title;
    const key = `${title}|${href}`;
    if (!seen.has(key)) {
      seen.add(key);
      entries.push({ title, text, href });
    }
  }

  function render(query = "") {
    const needle = query.trim().toLocaleLowerCase();
    const matches = entries.filter((entry) => !needle || entry.text.toLocaleLowerCase().includes(needle)).slice(0, 12);
    results.replaceChildren(...matches.map((entry) => {
      const item = document.createElement("li");
      const link = document.createElement("a");
      link.href = entry.href;
      const title = document.createElement("strong");
      title.textContent = entry.title;
      const context = document.createElement("span");
      context.textContent = entry.text.slice(0, 150);
      link.append(title, context);
      link.addEventListener("click", () => dialog.close());
      item.append(link);
      return item;
    }));
    count.textContent = matches.length ? `${matches.length} result${matches.length === 1 ? "" : "s"}` : "No matching documentation";
  }

  function openSearch() {
    dialog.showModal();
    render(input.value);
    input.focus();
    input.select();
  }

  trigger.addEventListener("click", openSearch);
  input.addEventListener("input", () => render(input.value));
  document.addEventListener("keydown", (event) => {
    const editing = event.target instanceof HTMLElement && (event.target.isContentEditable || /INPUT|TEXTAREA|SELECT/.test(event.target.tagName));
    if (event.key === "/" && !editing && !dialog.open) {
      event.preventDefault();
      openSearch();
    }
    if (event.key === "Escape" && dialog.open) dialog.close();
  });
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });
  dialog.addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    const focusable = [...dialog.querySelectorAll('button:not([disabled]), input:not([disabled]), a[href]')];
    if (!focusable.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  });
  dialog.addEventListener("close", () => trigger.focus());
})();
