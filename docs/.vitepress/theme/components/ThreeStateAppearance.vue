<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from 'vue'

type Mode = 'system' | 'light' | 'dark'

const STORAGE_KEY = 'vitepress-theme-appearance'
const mode = ref<Mode>('system')
const menuOpen = ref(false)

let mql: MediaQueryList | null = null

function isSystemDark(): boolean {
  return !!mql?.matches
}

function applyDomClass(isDark: boolean) {
  document.documentElement.classList.toggle('dark', isDark)
}

function setMode(newMode: Mode) {
  if (newMode === 'system') {
    localStorage.removeItem(STORAGE_KEY)
    mode.value = 'system'
    applyDomClass(isSystemDark())
  } else if (newMode === 'light') {
    localStorage.setItem(STORAGE_KEY, 'light')
    mode.value = 'light'
    applyDomClass(false)
  } else {
    localStorage.setItem(STORAGE_KEY, 'dark')
    mode.value = 'dark'
    applyDomClass(true)
  }
  menuOpen.value = false
}

function toggleMenu() {
  menuOpen.value = !menuOpen.value
}

function closeMenu(e: MouseEvent) {
  const target = e.target as HTMLElement
  if (!target.closest('.theme-switcher')) {
    menuOpen.value = false
  }
}

function syncFromStorage() {
  const v = localStorage.getItem(STORAGE_KEY)
  if (v === 'dark') {
    mode.value = 'dark'
    applyDomClass(true)
  } else if (v === 'light') {
    mode.value = 'light'
    applyDomClass(false)
  } else {
    mode.value = 'system'
    applyDomClass(isSystemDark())
  }
}

let onChange: (() => void) | null = null

onMounted(() => {
  mql = window.matchMedia('(prefers-color-scheme: dark)')
  syncFromStorage()

  onChange = () => {
    if (mode.value === 'system') applyDomClass(isSystemDark())
  }

  if (mql.addEventListener) mql.addEventListener('change', onChange)
  else (mql as MediaQueryList).addListener(onChange)

  document.addEventListener('click', closeMenu)
})

onBeforeUnmount(() => {
  if (!mql || !onChange) return
  if (mql.removeEventListener) mql.removeEventListener('change', onChange)
  else (mql as MediaQueryList).removeListener(onChange)

  document.removeEventListener('click', closeMenu)
})
</script>

<template>
  <div class="theme-switcher">
    <button
      class="trigger"
      type="button"
      @click="toggleMenu"
      :aria-expanded="menuOpen"
      aria-haspopup="menu"
      title="Change theme"
    >
      <!-- Sun icon (day mode) -->
      <svg v-if="mode === 'light'" xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="4"/>
        <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/>
      </svg>
      <!-- Moon icon (night mode) -->
      <svg v-else-if="mode === 'dark'" xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>
      </svg>
      <!-- System icon (auto mode) -->
      <svg v-else xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="2" y="3" width="20" height="14" rx="2"/>
        <path d="M8 21h8M12 17v4"/>
      </svg>
    </button>

    <div v-show="menuOpen" class="menu" role="menu">
      <button
        class="menu-item"
        :class="{ active: mode === 'system' }"
        type="button"
        role="menuitem"
        @click="setMode('system')"
      >
        <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <rect x="2" y="3" width="20" height="14" rx="2"/>
          <path d="M8 21h8M12 17v4"/>
        </svg>
        <span>System</span>
      </button>

      <button
        class="menu-item"
        :class="{ active: mode === 'light' }"
        type="button"
        role="menuitem"
        @click="setMode('light')"
      >
        <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="4"/>
          <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/>
        </svg>
        <span>Day</span>
      </button>

      <button
        class="menu-item"
        :class="{ active: mode === 'dark' }"
        type="button"
        role="menuitem"
        @click="setMode('dark')"
      >
        <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>
        </svg>
        <span>Night</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.theme-switcher {
  position: relative;
  display: flex;
  align-items: center;
}

.trigger {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 36px;
  height: 36px;
  border: 0;
  border-radius: 8px;
  cursor: pointer;
  color: var(--vp-c-text-2);
  background: transparent;
  transition: color 0.2s, background-color 0.2s;
}

.trigger:hover {
  color: var(--vp-c-text-1);
  background: var(--vp-c-bg-soft);
}

.menu {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  min-width: 128px;
  padding: 4px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  background: var(--vp-c-bg-elv);
  box-shadow: var(--vp-shadow-3);
  z-index: 100;
}

.menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 12px;
  border: 0;
  border-radius: 6px;
  cursor: pointer;
  font: inherit;
  font-size: 13px;
  color: var(--vp-c-text-2);
  background: transparent;
  transition: color 0.2s, background-color 0.2s;
}

.menu-item:hover {
  color: var(--vp-c-text-1);
  background: var(--vp-c-bg-soft);
}

.menu-item.active {
  color: var(--vp-c-brand-1);
}

.menu-item svg {
  flex-shrink: 0;
}
</style>
