(function(g){
'use strict';
const district={start:'2026-08-06',end:'2027-05-27',source:'https://www.rjuhsd.us/fs/resource-manager/view/677c1b31-c31f-45fe-9341-d52020756763',closures:[
 ['2026-09-07','2026-09-07','Labor Day'],['2026-11-02','2026-11-02','Staff development — no school'],
 ['2026-11-11','2026-11-11','Veterans Day'],['2026-11-23','2026-11-27','Thanksgiving break'],
 ['2026-12-21','2027-01-01','Winter break'],['2027-01-04','2027-01-04','Teacher preparation — no school'],
 ['2027-01-18','2027-01-18','Martin Luther King Jr. Day'],['2027-02-15','2027-02-19',"Presidents' week"],
 ['2027-03-22','2027-03-29','Spring break']
]};
const labels={regular:'Regular schedule',late:'Late start',pack3:'Late start · Period 3 Pack',pack4:'Late start · Period 4 Pack',nonpack:'Non-Pack day',minimum:'Minimum day',rally:'Rally schedule',exams:'Exam schedule'};
function resolve(school,date,events=[],mode='auto',period0=false,override=null){
 const schoolData=g.RJUHSD_SCHOOL_DATA[school];
 if(!schoolData)throw Error('Unknown school');
 if(mode==='auto'&&override&&override.date===date&&(!override.school||override.school===school||school==='woodcreek')&&override.schedule){
  const l1=(override.schedule.lunch1||[]).filter(p=>period0||p.name!=='Period 0');
  const l2=(override.schedule.lunch2||l1).filter(p=>period0||p.name!=='Period 0');
  const name=override.name||'Special schedule';
  const evt=override.special_event||name;
  return {date,type:'special',title:name,event:evt,blurb:override.special_event?`${name} · ${override.special_event}`:`${name} schedule active.`,inSession:l1.length>0,unavailable:false,manual:false,oneLunch:JSON.stringify(l1)===JSON.stringify(l2),lunch1:l1,lunch2:l2,source:'Special override'};
 }
 const day=new Date(date+'T12:00:00Z').getUTCDay(),descriptions=events.filter(e=>e.date===date).map(e=>String(e.text||e.title||''));
 const closed=district.closures.find(([a,b])=>date>=a&&date<=b);
 const eventClosure=descriptions.find(t=>/\bno school\b|\bnon.student day\b|\bschool closed\b/i.test(t));
 let reason=closed?.[2]||eventClosure||([0,6].includes(day)?'Weekend — no school':null);
 let type=day===3?'late':'regular',notice='',unknown=false;
 const joined=descriptions.join(' · ');
 if(/(?:collaboration|late.start).*3rd.*pack|3rd.*pack.*(?:collaboration|late.start)/i.test(joined))type='pack3';
 else if(/(?:collaboration|late.start).*4th.*pack|4th.*pack.*(?:collaboration|late.start)/i.test(joined))type='pack4';
 else if(/non.pack|no pack/i.test(joined))type='nonpack';
 else if(/minimum (?:day|bell)|minimum.*schedule/i.test(joined))type='minimum';
 else if(/rally (?:day|bell|schedule)|(?:bell|schedule).*rally/i.test(joined))type='rally';
 else if(/\bmidterms?\b|\bfinal exams?\b|\bfinals\b/i.test(joined))type='exams';
 else if(/late.start|collaboration|intervention.*schedule|panther period.*schedule/i.test(joined))type='late';
 else if(/regular (?:day|bell).*schedule/i.test(joined))type='regular';
 else if(/special (?:day|bell|schedule)|modified (?:day|bell|schedule)|caaspp.*(?:bell|schedule)/i.test(joined)){unknown=true;notice='Special schedule not verified — check the official school announcement';}
 if(date<district.start||date>district.end){reason=null;unknown=true;notice='Date outside the verified 2026–27 calendar — check the official school calendar';}
 if(mode!=='auto') {type=mode;reason=null;unknown=false;notice='Manual schedule preview — not an automatic calendar selection';}
 const pair=type==='regular'?schoolData.days[day===0||day===3||day===6?1:day]:schoolData.specials[type];
 if(!pair&&!reason){unknown=true;notice='Schedule not published — check the official school announcement';}
 const blocks=l=>!reason&&!unknown?(pair[l]||[]).filter(p=>period0||p.name!=='Period 0'):[];
 const lunch1=blocks(1),lunch2=blocks(2),inSession=!reason&&!unknown&&!!lunch1.length;
 return {date,type,title:reason||labels[type]||'Schedule unavailable',event:reason||(unknown?notice:''),blurb:notice||(type==='exams'?'Exam block times; use your school announcement for period assignments.':reason||'Verified against the official bell schedule.'),inSession,unavailable:unknown,manual:mode!=='auto',oneLunch:JSON.stringify(lunch1)===JSON.stringify(lunch2),lunch1,lunch2,source:schoolData.source};
}
g.RJUHSD_CALENDAR={district,labels,resolve};
})(window);
