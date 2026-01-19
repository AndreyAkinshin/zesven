import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import ThreeStateAppearance from './components/ThreeStateAppearance.vue'
import Layout from './Layout.vue'
import './custom.css'

export default {
  extends: DefaultTheme,
  Layout,
  enhanceApp({ app }) {
    app.component('ThreeStateAppearance', ThreeStateAppearance)
  },
} satisfies Theme
