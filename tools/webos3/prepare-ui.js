/* Generate an ES5 candidate UI without changing the upstream source. */
'use strict';
var fs = require('fs');
var path = require('path');
var ts = require('typescript');
var root = path.resolve(__dirname, '../..');
var dest = path.join(root, 'dist', 'webos3');
function cp(source, target) {
  fs.mkdirSync(target, { recursive: true });
  fs.readdirSync(source, { withFileTypes: true }).forEach(function (entry) {
    var from = path.join(source, entry.name), to = path.join(target, entry.name);
    if (entry.isDirectory()) cp(from, to);
    else fs.copyFileSync(from, to);
  });
}
function change(file, callback) {
  var before = fs.readFileSync(file, 'utf8');
  var after = callback(before);
  if (before === after) throw new Error('Expected marker not found in ' + file);
  fs.writeFileSync(file, after);
}
function transpile(file) {
  var before = fs.readFileSync(file, 'utf8');
  var result = ts.transpileModule(before, {
    fileName: path.basename(file), reportDiagnostics: true,
    compilerOptions: { target: ts.ScriptTarget.ES5, module: ts.ModuleKind.CommonJS,
      removeComments: false, newLine: ts.NewLineKind.LineFeed }
  });
  var errs = (result.diagnostics || []).filter(function (d) { return d.category === ts.DiagnosticCategory.Error; });
  if (errs.length) throw new Error('TypeScript compile errors: ' + errs.map(function (d) { return d.messageText; }).join('; '));
  fs.writeFileSync(file, result.outputText);
}
['ui','webos'].forEach(function (name) { cp(path.join(root,name), path.join(dest,name)); });
['ui','webos'].forEach(function (name) {
  fs.copyFileSync(path.join(__dirname,'compat-webos3.js'), path.join(dest,name,'compat-webos3.js'));
});
['app.js','i18n.js'].forEach(function (file) { transpile(path.join(dest,'ui',file)); });
transpile(path.join(dest,'webos','launch.js'));
change(path.join(dest, 'ui', 'app.js'), function (js) {
  var marker = 'var key = event.key, editing =';
  if (js.indexOf(marker) < 0) throw new Error('Expected keyboard handler not found');
  return js.replace(marker, "var key = (event.key && event.key !== 'Unidentified') ? event.key : ({37:'ArrowLeft',38:'ArrowUp',39:'ArrowRight',40:'ArrowDown',13:'Enter',27:'Escape',9:'Tab',32:' '})[event.keyCode], editing =")
    .replace('candidate.focus({ preventScroll: true });', 'candidate.focus();')
    .replace("candidate.scrollIntoView({ block: 'nearest', inline: 'nearest' });", 'candidate.scrollIntoView(false);');
});
fs.copyFileSync(path.join(dest,'ui','i18n.js'), path.join(dest,'webos','i18n.js'));
change(path.join(dest,'ui','index.html'), function (html) {
  return html.replace('<script src="i18n.js"></script>', '<script src="compat-webos3.js"></script>\n<script src="i18n.js"></script>');
});
change(path.join(dest,'webos','index.html'), function (html) {
  return html.replace('<script src="i18n.js"></script>', '<script src="compat-webos3.js"></script><script src="i18n.js"></script>');
});
change(path.join(dest,'webos','index.html'), function (html) {
  return html.replace('display:grid;place-items:center;', 'display:flex;align-items:center;justify-content:center;');
});
fs.appendFileSync(path.join(dest,'ui','styles.css'), '\n' + fs.readFileSync(path.join(__dirname,'compat-webos3.css'),'utf8'));
console.log('Generated experimental Chromium 38 UI in dist/webos3/ui and dist/webos3/webos');
