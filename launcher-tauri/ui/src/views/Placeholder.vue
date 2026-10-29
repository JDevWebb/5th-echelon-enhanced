<template>
  <div class="page">
    <div class="inner">
      <p class="kicker">{{ kicker }}</p>
      <h1 class="title">{{ title }}</h1>
      <p class="soft">{{ text }}</p>
      <div class="card note">
        <p class="caps">Moving over</p>
        <p class="muted">This screen comes across from the egui launcher in the next step of 0.4.8. Until then, the egui launcher has it.</p>
      </div>
    </div>
  </div>
</template>

<script setup>
// A screen not moved over yet (the Tauri launcher's step 2).
defineProps({ kicker: String, title: String, text: String });
</script>

<style scoped>
.note { margin-top: 18px; max-width: 640px; }
.note p { margin: 0 0 6px; }
</style>
