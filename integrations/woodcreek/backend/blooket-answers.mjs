import { PIN_SELECTOR, NAME_SELECTOR, visibleField, submitJoinField } from './blooket-connection.mjs';
const clean=value=>String(value??'').replace(/\s+/g,' ').trim().toLowerCase();

// Submit through the visible game control once; never click the next screen blindly.
export async function submitAnswer(page,state,choice,active=()=>true) {
  if(!active())return null;
  if(await visibleField(page,PIN_SELECTOR).count()||await visibleField(page,NAME_SELECTOR).count())return null;
  const heading=page.locator('[class*="questionText"], [class*="QuestionText"], [class*="questionHolder"], [class*="questionContainer"]').filter({visible:true}).first();
  if(await heading.count()) {
    const current=clean(await heading.innerText());
    if(current&&state.questionText&&current!==clean(state.questionText))return null;
  }
  if(state.questionType==='typing') {
    const input=page.locator('input[class*="typingInput"], input[placeholder*="type" i], input[placeholder*="answer" i]').filter({visible:true}).first();
    if(!await input.count())return null;
    if(!active())return null;
    await input.fill(choice.text);if(!active())return null;await submitJoinField(input);
    return {text:choice.text};
  }
  for(const selector of ['[class*="answerContainer"]','[class*="answerWrapper"]','[class*="answersHolder"] > div','[class*="answersHolder"] > button','[class*="answersWrapper"] > div','[class*="answerCard"]','[role="button"][class*="answer"]']) {
    const cards=page.locator(selector).filter({visible:true});
    const count=await cards.count();if(count<2)continue;
    const texts=await cards.allInnerTexts();
    const index=texts.findIndex(text=>clean(text)===clean(choice.text));
    if(index<0)return null;
    if(!active())return null;
    await cards.nth(index).click({timeout:2000});
    return {text:texts[index].trim()};
  }
  return null;
}

export async function advanceFeedback(page) {
  for(const selector of ['[class*="nextButton"]','[class*="feedbackContainer"]','[class*="feedbackWrapper"]','[class*="feedback"]']) {
    const control=page.locator(selector).filter({visible:true}).first();
    if(await control.count()){await control.click({timeout:1500});return true;}
  }
  const prompt=page.getByText(/^(?:click|press) anywhere (?:to (?:continue|go next)|to cast)|^(?:continue|go next|claim|collect)$/i).filter({visible:true}).first();
  if(await prompt.count()){await prompt.click({timeout:1500});return true;}
  return false;
}
