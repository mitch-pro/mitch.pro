(function () {
  if (window.MitchPeople) return;
  const activity = person => !person.online ? 0 : person.status === 'active' ? 3 : person.status === 'away' ? 1 : 2;
  window.MitchPeople = {
    compare(a, b) { return activity(b) - activity(a) || Number(b.lastActive || 0) - Number(a.lastActive || 0) || String(a.displayName || a.handle || '').localeCompare(String(b.displayName || b.handle || '')); },
    rank(person) { return ({'owner/developer':'Owner', 'co-owner/developer':'Co-owner', 'admin/developer':'Admin', moderator:'Moderator', contributor:'Contributor', premium:'Premium', member:'Member'})[person.role] || 'Member'; }
  };
})();
