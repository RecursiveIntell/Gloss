import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { NotebookDeleteConfirmation } from './NotebookSidebar';

describe('notebook deletion review surface', () => {
  it('names the notebook, affected content and an explicit cancellation action before deletion', () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const html = renderToStaticMarkup(createElement(NotebookDeleteConfirmation, { name: 'Research', sourceCount: 7, pending: false, onConfirm, onCancel }));
    expect(html).toContain('Confirm deletion of notebook Research');
    expect(html).toContain('7 sources, chats and notes');
    expect(html).toContain('This cannot be undone');
    expect(html).toContain('aria-label="Cancel notebook deletion"');
    expect(html).toContain('aria-label="Confirm delete notebook Research"');
    expect(onConfirm).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
  });
  it('disables both confirmation actions while the delete operation is pending', () => {
    const html = renderToStaticMarkup(createElement(NotebookDeleteConfirmation, { name: 'Research', sourceCount: 7, pending: true, onConfirm: vi.fn(), onCancel: vi.fn() }));
    expect(html.match(/<button[^>]*disabled=""[^>]*>/g)).toHaveLength(2);
    expect(html).toContain('Deleting…');
  });
});
