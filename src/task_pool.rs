//! Task Pool for local task caching and management
//!
//! Provides local task buffering to reduce API calls and handle 429 rate limiting

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::task::Task;

/// 本地任务池，缓存从服务器批量获取的任务
#[derive(Debug, Clone)]
pub struct TaskPool {
    /// 任务队列
    tasks: Arc<Mutex<VecDeque<Task>>>,
    /// 任务池容量
    capacity: usize,
    /// 已完成任务ID缓存，避免重复处理
    completed_tasks: Arc<Mutex<VecDeque<String>>>,
    /// 已完成任务缓存容量
    completed_capacity: usize,
}

impl TaskPool {
    /// 创建新的任务池
    pub fn new(capacity: usize) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
            completed_tasks: Arc::new(Mutex::new(VecDeque::with_capacity(500))),
            completed_capacity: 500,
        }
    }

    /// 获取当前任务池中的任务数量
    pub async fn len(&self) -> usize {
        self.tasks.lock().await.len()
    }

    /// 检查任务池是否为空
    #[allow(dead_code)]
    pub async fn is_empty(&self) -> bool {
        self.tasks.lock().await.is_empty()
    }

    /// 批量添加任务到池中
    pub async fn add_tasks(&self, new_tasks: Vec<Task>) -> usize {
        let mut tasks = self.tasks.lock().await;
        let completed = self.completed_tasks.lock().await;
        let mut added = 0;
        
        for task in new_tasks {
            // 检查是否已完成过此任务
            if completed.iter().any(|id| id == &task.task_id) {
                continue; // 跳过已完成的任务
            }
            
            if tasks.len() < self.capacity {
                tasks.push_back(task);
                added += 1;
            } else {
                break; // 池已满
            }
        }
        added
    }

    /// 从池中获取下一个任务
    pub async fn get_next_task(&self) -> Option<Task> {
        self.tasks.lock().await.pop_front()
    }

    /// 标记任务为已完成
    pub async fn mark_task_completed(&self, task_id: String) {
        let mut completed = self.completed_tasks.lock().await;
        
        // 如果缓存满了，移除最旧的记录
        if completed.len() >= self.completed_capacity {
            completed.pop_front();
        }
        
        completed.push_back(task_id);
    }

    /// 检查是否需要获取更多任务
    pub async fn should_fetch_more(&self, threshold: usize) -> bool {
        self.len().await < threshold
    }

    /// 清空任务池（用于错误恢复）
    #[allow(dead_code)]
    pub async fn clear(&self) {
        self.tasks.lock().await.clear();
    }

    /// 获取任务池状态信息
    #[allow(dead_code)]
    pub async fn get_status(&self) -> TaskPoolStatus {
        let task_count = self.len().await;
        let completed_count = self.completed_tasks.lock().await.len();
        
        TaskPoolStatus {
            task_count,
            completed_count,
            capacity: self.capacity,
        }
    }
}

/// 任务池状态信息
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TaskPoolStatus {
    pub task_count: usize,
    pub completed_count: usize,
    pub capacity: usize,
}

impl TaskPoolStatus {
    /// 计算任务池使用率
    #[allow(dead_code)]
    pub fn usage_percentage(&self) -> f64 {
        if self.capacity > 0 {
            (self.task_count as f64 / self.capacity as f64) * 100.0
        } else {
            0.0
        }
    }
} 