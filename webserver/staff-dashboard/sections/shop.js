/* Shop catalog overrides. Admins only. Changes stay local until "Save changes". */
(function () {
  'use strict';
  var SD = window.SD, h = SD.h, ui = SD.ui;

  async function render(root) {
    var catalog = (await SD.api('/api/admin/shop/catalog')).catalog || [];
    var tableBox = h('div');
    var status = h('p', { class: 'sd-form-status', role: 'status' });
    function say(t, bad) { status.className = 'sd-form-status' + (bad ? ' err' : t ? ' ok' : ''); status.textContent = t || ''; }

    function draw() {
      tableBox.textContent = '';
      tableBox.appendChild(ui.table(['ID', 'Name', 'Section', 'Type', 'Cost', 'Description', ''], catalog.map(function (it, i) {
        function edit(key, type) {
          var inp = h('input', { class: 'sd-input', type: type || 'text', value: it[key] == null ? '' : it[key] });
          inp.addEventListener('change', function () { it[key] = type === 'number' ? (parseInt(inp.value, 10) || 0) : inp.value; });
          return inp;
        }
        return [
          h('code', { text: it.id }), edit('name'), it.section || '',
          (it.type || '') + (it.costType ? ' (' + it.costType + ')' : '') + (it.premiumOnly ? ' · premium' : '') + (it.adminOnly ? ' · admin' : ''),
          edit('cost', 'number'), edit('desc'),
          ui.button('Delete', 'danger', function () { catalog.splice(i, 1); draw(); say('Item removed. Save changes to keep it.'); }),
        ];
      }), 'No items in the catalog.'));
    }
    draw();

    var save = ui.button('Save changes', 'primary', async function (b) {
      b.disabled = true; say('Saving...');
      try { var d = await SD.post('/api/admin/shop/catalog/save', { catalog: catalog }); say(d.pending ? SD.pendingText(d) : 'Shop catalog saved.'); }
      catch (e) { say(e.message, true); }
      b.disabled = false;
    });
    var reset = ui.button('Reset to defaults', 'danger', async function (b) {
      if (!(await SD.confirm('Reset catalog', 'Reset every shop item to the server defaults? This clears all overrides.', { danger: true, ok: 'Reset' }))) return;
      b.disabled = true;
      try { await SD.post('/api/admin/shop/catalog/save', { catalog: [] }); SD.show(); }
      catch (e) { say(e.message, true); b.disabled = false; }
    });
    root.appendChild(h('div', { class: 'sd-actions' }, [save, reset]));
    root.appendChild(status);
    root.appendChild(tableBox);

    root.appendChild(ui.heading('Add a cosmetic'));
    root.appendChild(ui.grid([ui.form({
      title: 'New item', desc: 'Adds to the table above. Save changes to publish it.', submitLabel: 'Add to table',
      fields: [
        { name: 'id', label: 'ID', required: true }, { name: 'name', label: 'Name', required: true },
        { name: 'section', label: 'Section', type: 'select', options: [['Name Colors', 'Name Colors'], ['Badges', 'Badges'], ['Themes', 'Themes'], ['Premium', 'Premium']] },
        { name: 'costType', label: 'Kind', type: 'select', options: [['name_color', 'name_color'], ['badge', 'badge'], ['theme', 'theme'], ['premium', 'premium']] },
        { name: 'cost', label: 'Cost (coins)', type: 'number', min: 0, value: 0 }, { name: 'desc', label: 'Description', required: true },
        { name: 'premiumOnly', label: 'Premium only', type: 'checkbox' }, { name: 'adminOnly', label: 'Admin only', type: 'checkbox' },
      ],
      submit: function (v) {
        if (catalog.some(function (it) { return it.id === v.id; })) throw new Error('An item with the ID "' + v.id + '" already exists.');
        catalog.push({ id: v.id, name: v.name, section: v.section, type: 'cosmetic', costType: v.costType, cost: v.cost || 0, desc: v.desc, premiumOnly: v.premiumOnly, adminOnly: v.adminOnly });
        draw();
        return 'Added to the table. Save changes to publish it.';
      },
    })]));
  }

  SD.register({ key: 'shop', render: render });
})();
