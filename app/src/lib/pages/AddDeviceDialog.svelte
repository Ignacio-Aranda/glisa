<script lang="ts">
  import Dialog from '../components/Dialog.svelte';
  import Button from '../components/Button.svelte';
  import Icon from '../components/Icon.svelte';
  import { shell } from '../backend';
  import { toast } from '../state/app.svelte';
  import { t } from '../i18n/index.svelte';

  interface Props { open: boolean; onclose: () => void }
  let { open, onclose }: Props = $props();

  async function openBluetooth() {
    try {
      await shell.openBluetooth();
    } catch {
      toast(t("Couldn't open the Bluetooth settings"), 'error');
    }
  }
</script>

<Dialog {open} {onclose} title={t('Add a device')} icon="plus" width={560}
  description={t('{app} finds the Logitech devices connected to this computer by itself. Connect yours in one of these two ways and it will show up here within seconds.')}>
  <section class="way">
    <h3><Icon name="bluetooth" size={18} /> {t('Via Bluetooth')}</h3>
    <ol>
      <li>{t('Turn on the device and hold down its pairing button (the channel button) until the light blinks quickly')}</li>
      <li>{t('Open the Bluetooth settings of your system and pick the device from the list')}</li>
      <li>{t('Come back to {app}: there is nothing else to do')}</li>
    </ol>
    <div><Button icon="external-link" onclick={openBluetooth}>{t('Open Bluetooth settings')}</Button></div>
  </section>
  <section class="way">
    <h3><Icon name="usb" size={18} /> {t('With a USB receiver')}</h3>
    <p class="muted">{t('Plug the receiver into this computer and turn on the device. If they came together in the box, they are already paired.')}</p>
  </section>
  {#snippet footer()}
    <Button variant="primary" onclick={onclose} data-autofocus>{t('Got it')}</Button>
  {/snippet}
</Dialog>

<style>
  .way {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-4) var(--space-5);
    border-radius: var(--radius-m);
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  h3 {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-md);
  }
  h3 :global(.icon) {
    color: var(--accent);
  }
  ol {
    margin: 0;
    padding-left: var(--space-5);
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    color: var(--text-muted);
  }
  ol li::marker {
    color: var(--accent);
    font-weight: 700;
  }
</style>
