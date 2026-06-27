use crate::MenuItem;
use alloc::string::String;
use alloc::vec::Vec;

/// 菜单状态
#[derive(Debug, Clone)]
pub struct MenuState {
    /// 所有菜单项
    pub items: Vec<MenuItem>,
    /// 当前选中索引
    pub selected: usize,
    /// 滚动偏移
    pub scroll_offset: usize,
    /// 是否需要重绘
    pub dirty: bool,
    /// 过滤器
    pub filter: String,
}

impl MenuState {
    /// 创建新菜单
    pub fn new(items: Vec<MenuItem>) -> Self {
        Self {
            items,
            selected: 0,
            scroll_offset: 0,
            dirty: true,
            filter: String::new(),
        }
    }

    /// 创建空菜单
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// 移动选择
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.dirty = true;
        }
    }

    /// 移动选择
    pub fn move_down(&mut self) {
        if self.selected < self.items.len().saturating_sub(1) {
            self.selected += 1;
            self.dirty = true;
        }
    }

    /// 移动到第一项
    pub fn move_first(&mut self) {
        if !self.items.is_empty() && self.selected != 0 {
            self.selected = 0;
            self.dirty = true;
        }
    }

    /// 移动到最后一项
    pub fn move_last(&mut self) {
        let last = self.items.len().saturating_sub(1);
        if self.selected != last {
            self.selected = last;
            self.dirty = true;
        }
    }

    /// 翻页
    pub fn page_up(&mut self, page_size: usize) {
        if page_size > 0 && self.selected > 0 {
            self.selected = self.selected.saturating_sub(page_size);
            self.dirty = true;
        }
    }

    /// 翻页
    pub fn page_down(&mut self, page_size: usize) {
        if page_size > 0 {
            let max = self.items.len().saturating_sub(1);
            self.selected = (self.selected + page_size).min(max);
            self.dirty = true;
        }
    }

    /// 获取当前选中项
    pub fn selected_item(&self) -> Option<&MenuItem> {
        self.items.get(self.selected)
    }

    /// 获取可显示范围
    pub fn visible_range(&mut self, max_items: usize) -> core::ops::Range<usize> {
        if self.items.is_empty() {
            return 0..0;
        }

        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + max_items {
            self.scroll_offset = self.selected - max_items + 1;
        }

        let end = (self.scroll_offset + max_items).min(self.items.len());
        self.scroll_offset..end
    }

    /// 设置过滤器
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_lowercase();
        self.selected = 0;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    /// 获取过滤后的项目
    pub fn filtered_items(&self) -> Vec<&MenuItem> {
        if self.filter.is_empty() {
            self.items.iter().collect()
        } else {
            self.items
                .iter()
                .filter(|item| item.label.to_lowercase().contains(&self.filter))
                .collect()
        }
    }

    /// 项目数量
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 添加项目
    pub fn add(&mut self, item: MenuItem) {
        self.items.push(item);
        self.dirty = true;
    }

    /// 清空项目
    pub fn clear(&mut self) {
        self.items.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    /// 排序项目 (按名称)
    pub fn sort_by_name(&mut self) {
        self.items.sort_by(|a, b| a.label.cmp(&b.label));
        self.dirty = true;
    }

    /// 排序项目 (按大小)
    pub fn sort_by_size(&mut self) {
        self.items.sort_by(|a, b| b.size.cmp(&a.size));
        self.dirty = true;
    }

    /// 排序项目 (按类型)
    pub fn sort_by_type(&mut self) {
        self.items
            .sort_by(|a, b| a.iso_type.display_name().cmp(b.iso_type.display_name()));
        self.dirty = true;
    }
}
