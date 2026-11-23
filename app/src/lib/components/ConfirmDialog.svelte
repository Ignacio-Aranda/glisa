<script lang="ts">
  import Dialog from './Dialog.svelte';
  import Button from './Button.svelte';
  import { confirmState, answer } from '../state/confirm.svelte';
  import { t } from '../i18n/index.svelte';

  const req = $derived(confirmState.request);
</script>

<Dialog
  open={!!req}
  title={req?.title ?? ''}
  icon={req?.icon ?? (req?.danger ? 'triangle-exclamation-point' : 'circle-question-mark')}
  iconTone={req?.danger ? 'danger' : 'accent'}
  width={460}
  onclose={() => answer(false)}
>
  {#if req}<p class="muted">{req.text}</p>{/if}
  {#snippet footer()}
    <Button onclick={() => answer(false)}>{t('Cancel')}</Button>
    <Button variant={req?.danger ? 'danger-solid' : 'primary'} onclick={() => answer(true)} data-autofocus>
      {req?.confirm ?? ''}
    </Button>
  {/snippet}
</Dialog>
