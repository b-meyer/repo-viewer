<template>
  <app-header />
  <main class="absolute inset-0 top-50 flex flex-col overflow-auto bg-gray-100">
    <router-view />
  </main>
</template>

<script setup lang="ts">
import AppHeader from './Header.vue';
import '@/styles/main.css';
</script>
