import sys
import json
import asyncio
from playwright.async_api import async_playwright
from duckduckgo_search import DDGS

async def comprehensive_search(query, max_results=5):
    results = []
    # 1. Search with DuckDuckGo (Faster/no API key)
    with DDGS() as ddgs:
        search_results = list(ddgs.text(query, max_results=max_results))
        
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        context = await browser.new_context(user_agent="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Safari/537.36")
        page = await context.new_page()

        for res in search_results:
            try:
                await page.goto(res['href'], timeout=10000)
                # Wait for main content
                await page.wait_for_load_state('networkidle')
                # Extract clean text
                content = await page.evaluate("""() => {
                    const sel = 'article, main, .content, #content, .post, body';
                    const el = document.querySelector(sel);
                    return el ? el.innerText : '';
                }""")
                
                results.append({
                    "title": res['title'],
                    "url": res['href'],
                    "snippet": res['body'],
                    "content": content[:5000] # Deep grab
                })
            except Exception:
                # Fallback to snippet if scrape fails
                results.append({
                    "title": res['title'],
                    "url": res['href'],
                    "snippet": res['body'],
                    "content": res['body']
                })
        
        await browser.close()
    return results

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(json.dumps({"error": "No query provided"}))
        sys.exit(1)
    
    query = sys.argv[1]
    results = asyncio.run(comprehensive_search(query))
    print(json.dumps(results))
