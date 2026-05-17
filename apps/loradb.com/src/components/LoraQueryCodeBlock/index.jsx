import React from 'react';
import BrowserOnly from '@docusaurus/BrowserOnly';
import {useColorMode} from '@docusaurus/theme-common';
import clsx from 'clsx';
import '@loradb/lora-query/styles.css';
import {useFormattedCypher} from '@site/src/lib/useFormattedCypher';
import styles from './styles.module.scss';

function normalizeSource(code, children) {
  if (typeof code === 'string') return code.replace(/\n$/, '');
  if (typeof children === 'string') return children.replace(/\n$/, '');
  if (Array.isArray(children)) {
    return children
      .filter((child) => typeof child === 'string')
      .join('')
      .replace(/\n$/, '');
  }
  return '';
}

function StaticFallback({source, className}) {
  return (
    <pre className={clsx(styles.fallback, className)}>
      <code className="language-cypher">{source}</code>
    </pre>
  );
}

function LoadedLoraQueryCodeBlock({source, colorScheme}) {
  const [editorModule, setEditorModule] = React.useState(null);
  // Reuse the shared formatter hook so every Cypher surface on the
  // site auto-formats with identical semantics. The hook returns the
  // raw `source` until the WASM module resolves, then snaps to the
  // prettified text.
  const formattedSource = useFormattedCypher(source);

  React.useEffect(() => {
    let cancelled = false;
    import('@loradb/lora-query')
      .then(async (mod) => {
        if (mod.__tla) await mod.__tla;
        if (!cancelled) setEditorModule(mod);
      })
      .catch(() => {
        if (!cancelled) setEditorModule(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!editorModule?.LoraQueryEditor) {
    return <StaticFallback source={formattedSource} />;
  }

  const {LoraQueryEditor} = editorModule;
  const lineCount = Math.max(1, formattedSource.split('\n').length);
  const minHeight = `${Math.min(420, Math.max(58, lineCount * 22 + 28))}px`;

  return (
    <LoraQueryEditor
      value={formattedSource}
      readOnly
      colorScheme={colorScheme}
      showLineNumbers={lineCount > 1}
      minHeight={minHeight}
      maxHeight="460px"
      className={styles.editor}
    />
  );
}

export default function LoraQueryCodeBlock({code, children, className}) {
  const source = normalizeSource(code, children);
  const {colorMode} = useColorMode();
  const colorScheme = colorMode === 'dark' ? 'dark' : 'light';

  if (!source) {
    return <StaticFallback source="" className={className} />;
  }

  return (
    <div className={styles.root}>
      <BrowserOnly fallback={<StaticFallback source={source} className={className} />}>
        {() => (
          <LoadedLoraQueryCodeBlock
            source={source}
            colorScheme={colorScheme}
          />
        )}
      </BrowserOnly>
    </div>
  );
}
