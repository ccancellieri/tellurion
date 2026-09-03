/** Builds the shared link to the embedded UI dependency notices. */
export function thirdPartyNoticesLink(): HTMLElement {
  const footer = document.createElement('footer');
  footer.className = 'legal-notices';
  const link = document.createElement('a');
  link.href = './THIRD_PARTY_NOTICES.txt';
  link.textContent = 'Third-party notices';
  footer.append(link);
  return footer;
}
