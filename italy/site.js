const tabs = [...document.querySelectorAll("[data-workload-tab]")];
const panels = [...document.querySelectorAll("[data-workload-panel]")];

tabs.forEach((tab) => {
  tab.addEventListener("click", () => {
    const target = tab.dataset.workloadTab;
    tabs.forEach((item) => item.setAttribute("aria-selected", String(item === tab)));
    panels.forEach((panel) => {
      panel.hidden = panel.dataset.workloadPanel !== target;
    });
  });
});

document.querySelectorAll("[data-copy]").forEach((button) => {
  button.addEventListener("click", async () => {
    const code = document.getElementById(button.dataset.copy);
    if (!code) return;

    try {
      await navigator.clipboard.writeText(code.textContent.trim());
      const previous = button.textContent;
      button.textContent = "Copied";
      window.setTimeout(() => {
        button.textContent = previous;
      }, 1600);
    } catch {
      code.focus();
    }
  });
});
