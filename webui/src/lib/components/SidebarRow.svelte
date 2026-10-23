<script>
	import { Archive, CircleAlert, Circle, LoaderCircle } from '@lucide/svelte';

	/**
	 * @type {{
	 *   title: string,
	 *   active: boolean,
	 *   status: 'idle' | 'running' | 'approval' | 'archived',
	 *   onSelect: () => void
	 * }}
	 */
	let { title, active, status, onSelect } = $props();

	const STATUS_LABEL = {
		idle: 'Idle',
		running: 'Running',
		approval: 'Needs approval',
		archived: 'Archived'
	};
</script>

<li>
	<button
		type="button"
		onclick={onSelect}
		aria-current={active ? 'true' : undefined}
		class={`flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-sm ${active ? 'bg-sidebar-accent text-sidebar-accent-foreground' : 'text-sidebar-foreground hover:bg-sidebar-accent/60'}`}
	>
		<span class="flex size-3.5 shrink-0 items-center justify-center" role="img" aria-label={STATUS_LABEL[status]}>
			{#if status === 'running'}
				<LoaderCircle class="size-3.5 animate-spin text-muted-foreground" aria-hidden="true" />
			{:else if status === 'approval'}
				<CircleAlert class="size-3.5 text-warning" aria-hidden="true" />
			{:else if status === 'archived'}
				<Archive class="size-3.5 text-muted-foreground" aria-hidden="true" />
			{:else}
				<Circle class="size-2.5 text-muted-foreground/60" aria-hidden="true" />
			{/if}
		</span>
		<span class={`min-w-0 flex-1 truncate ${status === 'archived' ? 'text-muted-foreground' : ''}`}>{title}</span>
	</button>
</li>
