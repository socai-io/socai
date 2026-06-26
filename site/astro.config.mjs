import { defineConfig } from 'astro/config';

export default defineConfig({
  site: 'https://socai.io',
  output: 'static',
  markdown: {
    // The site is monochrome/light; Shiki's default github-dark theme fights
    // that and hard-codes a dark background on code blocks via inline styles.
    // Disable it so plain <pre><code> picks up our own .prose styling.
    syntaxHighlight: false,
  },
});
