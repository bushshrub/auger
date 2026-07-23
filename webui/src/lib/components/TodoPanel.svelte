<script>
	import { Circle, CircleDot, CircleCheck, ListTodo } from '@lucide/svelte';

	/** @type {{ todos: { id: number, title: string, status: 'pending' | 'in_progress' | 'done' }[] }} */
	let { todos } = $props();

	const done = $derived(todos.filter((t) => t.status === 'done').length);
</script>

<aside class="flex w-72 shrink-0 flex-col border-l border-border bg-sidebar max-lg:hidden">
	<div class="flex items-center gap-2 border-b border-border px-4 py-3">
		<ListTodo class="size-4 text-primary" aria-hidden="true" />
		<h2 class="font-mono text-sm font-bold tracking-tight text-foreground">todo</h2>
		<span class="ml-auto font-mono text-[10px] text-muted-foreground">
			{done}/{todos.length}
		</span>
	</div>

	<ol class="min-h-0 flex-1 overflow-y-auto p-2 auger-scroll">
		{#each todos as t (t.id)}
			<li class="flex items-start gap-2 rounded-md px-2 py-1.5">
				{#if t.status === 'done'}
					<CircleCheck class="mt-0.5 size-3.5 shrink-0 text-success" aria-hidden="true" />
				{:else if t.status === 'in_progress'}
					<CircleDot class="mt-0.5 size-3.5 shrink-0 text-primary" aria-hidden="true" />
				{:else}
					<Circle class="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
				{/if}
				<span
					class={`text-xs leading-snug ${
						t.status === 'done'
							? 'text-muted-foreground line-through'
							: t.status === 'in_progress'
								? 'text-foreground'
								: 'text-muted-foreground'
					}`}
				>
					{t.title}
				</span>
			</li>
		{/each}
	</ol>
</aside>
