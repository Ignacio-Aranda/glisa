<script lang="ts">
  import mark from '../brand/logo.svg';
  interface Props { size?: number }
  let { size = 24 }: Props = $props();
</script>

<img class="logo" src={mark} width={size} height={size} alt="" aria-hidden="true" draggable="false" />

<style>
  .logo {
    display: block;
    flex: none;
  }
</style>
