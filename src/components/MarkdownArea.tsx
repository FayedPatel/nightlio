import { forwardRef, useImperativeHandle, useRef } from 'react';
import {
  MDXEditor,
  headingsPlugin,
  listsPlugin,
  quotePlugin,
  thematicBreakPlugin,
  markdownShortcutPlugin,
  linkPlugin,
  linkDialogPlugin,
  imagePlugin,
  tablePlugin,
  codeBlockPlugin,
  codeMirrorPlugin,
  diffSourcePlugin,
  frontmatterPlugin,
  directivesPlugin,
  AdmonitionDirectiveDescriptor,
  toolbarPlugin,
  UndoRedo,
  BoldItalicUnderlineToggles,
  CodeToggle,
  CreateLink,
  InsertImage,
  InsertTable,
  InsertThematicBreak,
  ListsToggle,
  BlockTypeSelect,
  Separator
} from '@mdxeditor/editor';
import type { MDXEditorMethods } from '@mdxeditor/editor';
import '@mdxeditor/editor/style.css';
import useMediaQuery from '../hooks/useMediaQuery';

/**
 * Imperative surface consumers reach through the forwarded ref
 * (EntryView's markdownRef).
 */
export interface MarkdownAreaHandle {
  getMarkdown(): string;
  getInstance(): { setMarkdown(md: string): void } | null;
}

interface MarkdownAreaProps {
  initialMarkdown?: string;
  onChange?: (markdown: string) => void;
}

const MyComponent = forwardRef<MarkdownAreaHandle, MarkdownAreaProps>(({ initialMarkdown, onChange }, ref) => {
  const editorRef = useRef<MDXEditorMethods | null>(null);
  // Full toolbar wraps into a messy multi-row mess at 640px. Mobile keeps
  // only the essentials (bold/italic, lists, link); desktop keeps everything.
  const isMobile = useMediaQuery('(max-width: 640px)');

  useImperativeHandle(ref, () => ({
    getMarkdown: () => {
      return editorRef.current?.getMarkdown() || '';
    },
    getInstance: () => ({
      setMarkdown: (newValue: string) => {
        editorRef.current?.setMarkdown(newValue);
      }
    })
  }));

  return (
    <div style={{
      border: 'none',
      borderRadius: '16px',
      overflow: 'hidden',
      background: 'var(--bg-card)',
      boxShadow: 'var(--shadow-lg)'
    }}>
      <style>
        {`
          .mdx-editor .prose {
            text-align: left !important;
            color: var(--text) !important;
          }
          .mdx-editor .prose * {
            text-align: left !important;
            color: inherit !important;
          }
          .mdx-editor [data-editor-type="root"] {
            text-align: left !important;
            color: var(--text) !important;
          }
          .mdx-editor [contenteditable] {
            text-align: left !important;
            color: var(--text) !important;
          }
          .mdx-editor a { color: var(--accent-600) !important; }
          .mdx-editor code { color: var(--text) !important; }
          .mdx-editor pre { color: var(--text) !important; }
          /* Placeholder: MDXEditor gives the placeholder div the same class
             as the contenteditable ("prose") but no contenteditable
             attribute. Library CSS forces nowrap+ellipsis (single line);
             undo that so the two-part placeholder can render. Fade with
             opacity so it stays muted under every theme's --text without
             fighting the color !important rules above. */
          .mdx-editor .prose:not([contenteditable]) {
            white-space: normal !important;
            overflow: visible !important;
            text-overflow: clip !important;
            opacity: 0.5;
          }
          .mdx-editor .entry-placeholder-title {
            display: block;
            font-size: 1.6em;
            font-weight: 700;
            margin-bottom: 0.6em;
          }
        `}
      </style>
      <MDXEditor
        ref={editorRef}
        markdown={initialMarkdown ?? ''}
        {...(onChange ? { onChange } : {})}
        contentEditableClassName="prose"
        placeholder={
          <span className="entry-placeholder">
            <span className="entry-placeholder-title">How was your day?</span>
            Write about your thoughts, feelings, and experiences...
          </span>
        }
        plugins={[
          headingsPlugin(),
          listsPlugin(),
          quotePlugin(),
          thematicBreakPlugin(),
          markdownShortcutPlugin(),
          linkPlugin(),
          linkDialogPlugin(),
          imagePlugin(),
          tablePlugin(),
          codeBlockPlugin({ defaultCodeBlockLanguage: 'txt' }),
          codeMirrorPlugin({ codeBlockLanguages: { txt: 'Plain Text', js: 'JavaScript', css: 'CSS' } }),
          directivesPlugin({ directiveDescriptors: [AdmonitionDirectiveDescriptor] }),
          frontmatterPlugin(),
          diffSourcePlugin({ viewMode: 'rich-text', diffMarkdown: '' }),
          toolbarPlugin({
            toolbarContents: () =>
              isMobile ? (
                <>
                  <BoldItalicUnderlineToggles />
                  <Separator />
                  <ListsToggle />
                  <Separator />
                  <CreateLink />
                </>
              ) : (
                <>
                  <UndoRedo />
                  <Separator />
                  <BoldItalicUnderlineToggles />
                  <CodeToggle />
                  <Separator />
                  <BlockTypeSelect />
                  <Separator />
                  <CreateLink />
                  <InsertImage />
                  <Separator />
                  <ListsToggle />
                  <InsertTable />
                  <InsertThematicBreak />
                </>
              )
          })
        ]}
        className="mdx-editor"
      />
    </div>
  );
});

MyComponent.displayName = 'MarkdownArea';
export default MyComponent;
