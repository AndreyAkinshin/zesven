import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'zesven',
  description: 'Pure Rust 7z archive library and format specification',
  appearance: true,

  head: [
    ['meta', { name: 'theme-color', content: '#0080ff' }],
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/logo.svg' }],
  ],

  themeConfig: {
    logo: '/logo.svg',

    nav: [
      { text: 'Rust Manual', link: '/rs/' },
      { text: '7z Spec', link: '/7z/' },
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/AndreyAkinshin/zesven' },
    ],

    sidebar: {
      '/7z/': sidebarSpec(),
      '/rs/': sidebarRust(),
    },

    search: {
      provider: 'local',
    },

    editLink: {
      pattern: 'https://github.com/AndreyAkinshin/zesven/edit/main/docs/:path',
      text: 'Edit this page on GitHub',
    },

    footer: {
      message: 'Released under MIT OR Apache-2.0 License',
      copyright: 'Copyright 2026 Andrey Akinshin',
    },
  },

  markdown: {
    theme: {
      light: 'github-light',
      dark: 'github-dark',
    },
    lineNumbers: true,
  },
})

function sidebarSpec() {
  return [
    {
      text: 'Foundation',
      collapsed: false,
      items: [
        { text: 'Introduction', link: '/7z/' },
        { text: 'Philosophy', link: '/7z/00-philosophy' },
        { text: 'Glossary', link: '/7z/01-glossary' },
        { text: 'Archive Structure', link: '/7z/02-archive-structure' },
        { text: 'Signature Header', link: '/7z/03-signature-header' },
        { text: 'Data Encoding', link: '/7z/04-data-encoding' },
      ],
    },
    {
      text: 'Header Format',
      collapsed: false,
      items: [
        { text: 'Header Structure', link: '/7z/05-header-structure' },
        { text: 'Pack Info', link: '/7z/06-pack-info' },
        { text: 'Unpack Info', link: '/7z/07-unpack-info' },
        { text: 'Substreams Info', link: '/7z/08-substreams-info' },
        { text: 'Files Info', link: '/7z/09-files-info' },
      ],
    },
    {
      text: 'Codecs & Filters',
      collapsed: false,
      items: [
        { text: 'Compression Methods', link: '/7z/10-compression-methods' },
        { text: 'Filters', link: '/7z/11-filters' },
        { text: 'Encryption', link: '/7z/12-encryption' },
      ],
    },
    {
      text: 'Special Features',
      collapsed: false,
      items: [
        { text: 'Solid Archives', link: '/7z/13-solid-archives' },
        { text: 'Multi-Volume', link: '/7z/14-multi-volume' },
        { text: 'SFX Archives', link: '/7z/15-sfx-archives' },
      ],
    },
    {
      text: 'Metadata & Safety',
      collapsed: false,
      items: [
        { text: 'Timestamps & Attributes', link: '/7z/16-timestamps-attributes' },
        { text: 'Security', link: '/7z/17-security' },
        { text: 'Error Conditions', link: '/7z/18-error-conditions' },
      ],
    },
    {
      text: 'Appendices',
      collapsed: true,
      items: [
        { text: 'A: Property IDs', link: '/7z/appendix/a-property-ids' },
        { text: 'B: Method IDs', link: '/7z/appendix/b-method-ids' },
        { text: 'C: CRC Algorithm', link: '/7z/appendix/c-crc-algorithm' },
        { text: 'D: Compatibility', link: '/7z/appendix/d-compatibility' },
      ],
    },
  ]
}

function sidebarRust() {
  return [
    {
      text: 'Getting Started',
      collapsed: false,
      items: [
        { text: 'Quick Start', link: '/rs/' },
        { text: 'Cookbook', link: '/rs/cookbook' },
      ],
    },
    {
      text: 'Reading Archives',
      collapsed: false,
      items: [
        { text: 'Overview', link: '/rs/reading/' },
        { text: 'Opening Archives', link: '/rs/reading/opening-archives' },
        { text: 'Extracting Files', link: '/rs/reading/extracting' },
        { text: 'Selective Extraction', link: '/rs/reading/selective-extraction' },
        { text: 'Progress Callbacks', link: '/rs/reading/progress-callbacks' },
      ],
    },
    {
      text: 'Writing Archives',
      collapsed: false,
      items: [
        { text: 'Overview', link: '/rs/writing/' },
        { text: 'Creating Archives', link: '/rs/writing/creating-archives' },
        { text: 'Compression Options', link: '/rs/writing/compression-options' },
        { text: 'Solid Archives', link: '/rs/writing/solid-archives' },
        { text: 'Appending', link: '/rs/writing/appending' },
      ],
    },
    {
      text: 'Encryption',
      collapsed: false,
      items: [
        { text: 'Overview', link: '/rs/encryption/' },
        { text: 'Reading Encrypted', link: '/rs/encryption/reading-encrypted' },
        { text: 'Creating Encrypted', link: '/rs/encryption/creating-encrypted' },
      ],
    },
    {
      text: 'Streaming API',
      collapsed: false,
      items: [
        { text: 'Overview', link: '/rs/streaming/' },
        { text: 'Configuration', link: '/rs/streaming/config' },
        { text: 'Memory Management', link: '/rs/streaming/memory-management' },
      ],
    },
    {
      text: 'Async API',
      collapsed: false,
      items: [
        { text: 'Overview', link: '/rs/async/' },
        { text: 'Tokio Integration', link: '/rs/async/tokio-integration' },
        { text: 'Cancellation', link: '/rs/async/cancellation' },
      ],
    },
    {
      text: 'Advanced Topics',
      collapsed: true,
      items: [
        { text: 'Overview', link: '/rs/advanced/' },
        { text: 'Editing Archives', link: '/rs/advanced/editing' },
        { text: 'Multi-Volume', link: '/rs/advanced/multi-volume' },
        { text: 'Self-Extracting', link: '/rs/advanced/sfx' },
        { text: 'Archive Recovery', link: '/rs/advanced/recovery' },
        { text: 'WASM/Browser', link: '/rs/advanced/wasm' },
      ],
    },
    {
      text: 'Safety & Security',
      collapsed: false,
      items: [
        { text: 'Overview', link: '/rs/safety/' },
        { text: 'Path Safety', link: '/rs/safety/path-safety' },
        { text: 'Resource Limits', link: '/rs/safety/resource-limits' },
      ],
    },
    {
      text: 'Reference',
      collapsed: true,
      items: [
        { text: 'Feature Flags', link: '/rs/reference/feature-flags' },
        { text: 'Error Handling', link: '/rs/reference/error-handling' },
        { text: 'Platform Support', link: '/rs/reference/platform-support' },
      ],
    },
  ]
}
