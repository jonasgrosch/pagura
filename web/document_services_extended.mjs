import { FileScopeCode, ImageTransferCode, MetadataOwnerCode } from './protocol_constants.mjs';

const text = bytes => bytes === null ? '' : [...bytes.subarray(0, 128)].map(byte => byte >= 32 && byte <= 126 ? String.fromCharCode(byte) : `\\x${byte.toString(16).padStart(2, '0')}`).join('');

/** Optional source inspection uses the same document/dialog guard and byte downloader. */
export class ExtendedServices {
  constructor(view) {
    this.view = view; const $ = id => view.document.getElementById(id);
    this.form = $('document-service-source-form'); this.choice = $('document-service-source');
    this.object = $('document-service-object'); this.output = $('document-service-source-results'); this.serial = 0;
    this.form.addEventListener('submit', event => { event.preventDefault(); void this.show(); });
  }
  reset() { this.serial++; this.output.replaceChildren(); }
  async show() {
    const view = this.view, snapshot = { ...view.state() }, serial = view.serial, selection = ++this.serial;
    if (!view.current(snapshot, serial)) return;
    const current = () => selection === this.serial && view.current(snapshot, serial);
    this.output.replaceChildren(view.node('p', 'Loading source…'));
    try {
      const choice = this.choice.value, page = view.selectedPage ?? snapshot.page ?? 0;
      let object = null;
      if (['object-associated', 'metadata', 'image-object', 'thumbnail-object'].includes(choice)) {
        const match = /^(\d+):(\d+)$/.exec(this.object.value.trim());
        if (!match || Number(match[1]) > 0xffffffff || Number(match[2]) > 65535) throw new Error('Enter an object as number:generation.');
        object = { number: Number(match[1]), generation: Number(match[2]) };
      }
      const scopes = { catalog: { kind: FileScopeCode.Catalog }, associated: { kind: FileScopeCode.Associated },
        'page-associated': { kind: FileScopeCode.PageAssociated, page }, 'page-annotations': { kind: FileScopeCode.PageAnnotations, page },
        'object-associated': { kind: FileScopeCode.ObjectAssociated, object } };
      const owner = object ? { kind: MetadataOwnerCode.Object, object } : { kind: MetadataOwnerCode.Page, page };
      if (scopes[choice]) {
        const scope = scopes[choice], files = await snapshot.client.files(snapshot.handle, scope);
        if (!current()) return; this.output.replaceChildren();
        if (!files.length) this.output.append(view.node('p', 'No files in this source.'));
        for (const file of files) this.file(file, scope, snapshot, serial, current);
      } else if (choice === 'collection') {
        const collection = await snapshot.client.collection(snapshot.handle);
        if (!current()) return; this.output.replaceChildren();
        if (!collection) { this.output.append(view.node('p', 'No collection declared.')); return; }
        this.output.append(view.node('p', `Initial view: ${collection.view}. Initial file key: ${text(collection.initialFile)}`));
        for (const field of collection.fields) this.output.append(view.node('p', `${field.name} (${field.kind}); ${field.visible ? 'visible' : 'hidden'}.`));
        for (const sort of collection.sort) this.output.append(view.node('p', `Sort ${text(sort.key)}: ${sort.ascending ? 'ascending' : 'descending'}.`));
      } else if (choice === 'metadata') {
        const packet = await snapshot.client.metadataFor(snapshot.handle, owner);
        if (!current()) return; this.output.replaceChildren();
        if (packet) this.output.append(this.button('Download object metadata packet', snapshot, serial, current, handle => snapshot.client.metadataBytesFor(handle, owner), `object-${object.number}-${object.generation}-metadata.xmp`));
        else this.output.append(view.node('p', 'No metadata packet on this object.'));
      } else {
        const thumbnailOnly = choice === 'thumbnail-object';
        const assets = thumbnailOnly ? [] : await snapshot.client.images(snapshot.handle, owner);
        const thumbnail = choice === 'image-object' ? null : await snapshot.client.thumbnail(snapshot.handle, owner);
        if (!current()) return; this.output.replaceChildren();
        this.output.append(view.node('p', 'These are source assets. Binary transfers retain PDF colour and mask dependencies; they are not rendered page images.'));
        for (const asset of assets) this.image(asset, asset.object, snapshot, serial, current);
        if (thumbnail) {
          const identity = object ?? { page };
          this.image(thumbnail, identity, snapshot, serial, current);
        }
        if (!assets.length && !thumbnail) this.output.append(view.node('p', 'No assets in this source.'));
      }
    } catch (error) { if (current()) this.output.replaceChildren(view.node('p', `Source unavailable: ${error}`)); }
  }
  button(label, snapshot, serial, current, read, name) {
    return this.view.button(label, snapshot, serial, async handle => {
      if (!current()) throw new Error('Source selection changed');
      const value = await read(handle); if (!current()) throw new Error('Source selection changed'); return value;
    }, name);
  }
  file(file, scope, snapshot, serial, current) {
    const view = this.view, item = view.document.createElement('section');
    item.append(view.node('p', file.names.UF ?? text(file.names.F) ?? `File ${file.index + 1}`));
    if (file.description) item.append(view.node('p', file.description));
    for (const [ordinal, payload] of file.payloads.entries()) {
      item.append(this.button(`Download file payload ${ordinal + 1}`, snapshot, serial, current,
        handle => snapshot.client.fileBytes(handle, scope, file.index, payload.key), `source-file-${file.index + 1}-payload-${ordinal + 1}.bin`));
      this.check(item, file, scope, payload.key, null, snapshot, serial, current);
    }
    for (const related of file.related) {
      item.append(view.node('p', `Related file: ${text(related.name)}`));
      item.append(this.button(`Download related file ${related.index + 1}`, snapshot, serial, current,
        handle => snapshot.client.fileBytes(handle, scope, file.index, related.key, related.index), `source-file-${file.index + 1}-related-${related.key}-${related.index + 1}.bin`));
      this.check(item, file, scope, related.key, related.index, snapshot, serial, current);
    }
    for (const field of file.collectionItem) item.append(view.node('p', `${text(field.key)}: ${field.prefix ?? ''}${field.value ?? ''}`));
    if (file.encryptedPayload) item.append(view.node('p', `Declared encrypted payload: ${text(file.encryptedPayload.subtype)} ${text(file.encryptedPayload.version)}`));
    this.output.append(item);
  }
  check(item, file, scope, key, related, snapshot, serial, current) {
    const view = this.view, button = view.node('button', related === null ? 'Check file payload' : `Check related file ${related + 1}`), result = view.node('p', 'Declarations have not been checked.');
    button.type = 'button'; view.buttons.push(button);
    button.addEventListener('click', () => {
      if (!current()) return;
      void view.checkResult(snapshot, serial, async handle => {
        const value = await snapshot.client.fileCheck(handle, scope, file.index, key, related);
        if (!current()) throw new Error('Source selection changed'); return value;
      }, result);
    }); item.append(button, result);
  }
  image(asset, identity, snapshot, serial, current) {
    const view = this.view, item = view.document.createElement('section');
    item.append(view.node('p', `${asset.thumbnail ? 'Stored thumbnail' : 'Image resource'} ${asset.object.number}:${asset.object.generation}, ${asset.width} × ${asset.height} samples`));
    for (const [label, mode] of [['Stored stream', ImageTransferCode.Stored], ['Encoded asset', ImageTransferCode.Encoded]]) {
      item.append(this.button(`Download ${label.toLowerCase()}`, snapshot, serial, current, async handle => {
        const result = await snapshot.client.imageBytes(handle, identity, mode, asset.thumbnail); return result.bytes;
      }, `image-${asset.object.number}-${asset.object.generation}-${label === 'Stored stream' ? 'stored' : 'encoded'}.bin`));
    }
    this.output.append(item);
  }
}
